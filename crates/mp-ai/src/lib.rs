//! The assistant: chat-completion clients for the three providers it supports, ported from
//! Recto's `ai.rs` (without its proposed-edit code).
//!
//! Requests are made from Rust, never from the extension, so API keys stay in Windows Credential
//! Manager and are read only here. Every provider is called over plain HTTPS. The model name is
//! whatever the reader typed, so tuning fields such as `temperature` are never sent: they differ
//! most between model generations, and omitting them keeps an arbitrary model name working.
//!
//! Two exceptions earn their keep. An output-token ceiling is sent to every provider, because a
//! model that reasons before it answers will otherwise spend an unbounded amount of time and can
//! exhaust its budget on reasoning and return no answer at all. OpenAI and Gemini also get a
//! control for how much the model reasons; both controls changed shape between model generations,
//! so a request rejected for that field is retried once without it.
//!
//! Every reply carries the provider's own token counts back to the caller, which is what makes a
//! daily free-tier allowance something the reader can watch rather than discover by being rate
//! limited.

mod context;
mod http;
mod store;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use context::{Context, link_citations, ranges};
pub use http::WinHttp;
pub use store::{Config, DailyUsage, content_hash, key, load_chat, save_chat, set_key, tokens};

/// Guard against oversized-request errors: roughly 75k tokens, which leaves room for the
/// conversation and the answer in the smaller context windows. Longer documents go through
/// page retrieval first (`Context`).
pub const MAX_DOC_CHARS: usize = 300_000;

/// Ceiling on what a model may produce for one answer. On reasoning models this budgets thinking
/// and answer text together.
const MAX_ANSWER_TOKENS: u32 = 16_000;

/// How much of the conversation travels with each request. The document is not counted against
/// this: it is a message of its own, rebuilt for every question.
const MAX_HISTORY_TOKENS: usize = 24_000;

/// The usual ratio for English prose. The budget exists to keep requests from growing without
/// bound, and erring low costs a little history rather than a failed request.
const CHARS_PER_TOKEN: usize = 4;

/// The assistant's half of the opening exchange that delivers the document.
const DOCUMENT_ACK: &str = "I have the document.";

const SYSTEM_PROMPT: &str = "You are assisting inside a PDF reader. The user's document is \
provided below as the text of its pages; each page opens with a tag such as [p. 12]. Answer \
their question about it, or produce the text they ask for. Cite the pages you draw on with the \
same tags, one page per tag, for example [p. 3] [p. 4]. Reply in Markdown using paragraphs, \
lists, bold and italics only: no headings, tables or code blocks. Keep answers tight: no \
preamble, no restating the question.";

/// What one exchange cost, as the provider counted it. Output includes tokens spent on
/// reasoning: every provider here bills those, and on a metered free tier they are exactly the
/// spend that is easiest to miss.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
}

#[derive(Debug)]
pub struct Answer {
    pub text: String,
    pub usage: Usage,
}

/// One message of a chat as it is kept: what was said, and who said it. `note` is shown under an
/// answer (the pages used, what it cost) and never sent to a provider. The document is never part
/// of a turn; it is a message of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub role: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl Turn {
    pub fn user(text: &str) -> Turn {
        Turn {
            role: "user".into(),
            text: text.into(),
            note: String::new(),
        }
    }

    pub fn assistant(text: &str, note: String) -> Turn {
        Turn {
            role: "assistant".into(),
            text: text.into(),
            note,
        }
    }
}

/// One message as a provider sees it. The role names differ between the three APIs, so they are
/// decided inside each request builder rather than here.
struct Msg {
    assistant: bool,
    content: String,
}

fn count(value: Option<&Value>, key: &str) -> u64 {
    value
        .and_then(|v| v.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Anthropic,
    OpenAi,
    Google,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::Anthropic, Provider::OpenAi, Provider::Google];

    pub fn from_id(id: &str) -> Provider {
        match id {
            "openai" => Provider::OpenAi,
            "google" => Provider::Google,
            _ => Provider::Anthropic,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAi => "openai",
            Provider::Google => "google",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic",
            Provider::OpenAi => "OpenAI",
            Provider::Google => "Google AI Studio",
        }
    }

    /// Shown as placeholder text so the expected shape of a model name is obvious. It is never
    /// used as a fallback: an unset model is an error, not something to guess at.
    pub fn example_model(self) -> &'static str {
        match self {
            Provider::Anthropic => "claude-haiku-4-5-20251001",
            Provider::OpenAi => "gpt-5.6-luna",
            Provider::Google => "gemini-3.7-flash",
        }
    }
}

/// Sends a JSON body and returns the status and the JSON reply (`Null` when the reply was not
/// JSON). `WinHttp` is the real one; tests stand in for the providers.
pub trait Transport {
    fn post(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &Value,
    ) -> Result<(u16, Value), String>;
}

/// Who answers: the provider chosen in ai.json, its model, and its key from Credential Manager.
#[derive(Debug, Clone, Default)]
pub struct Setup {
    pub provider: Provider,
    pub model: String,
    pub key: String,
}

impl Setup {
    pub fn load() -> Setup {
        let config = Config::load();
        let provider = config.provider;
        Setup {
            provider,
            model: config.model(provider).to_string(),
            key: key(provider).unwrap_or_default(),
        }
    }

    /// "Anthropic · claude-haiku-4-5", or the provider alone before a model is named.
    pub fn label(&self) -> String {
        match self.model.trim() {
            "" => self.provider.label().to_string(),
            model => format!("{} · {model}", self.provider.label()),
        }
    }

    /// What is missing before the assistant can answer; empty when nothing is.
    pub fn notice(&self) -> String {
        let label = self.provider.label();
        match (self.key.trim().is_empty(), self.model.trim().is_empty()) {
            (false, false) => String::new(),
            (true, true) => format!("The assistant needs an API key and a model name for {label}."),
            (true, false) => format!("The assistant needs an API key for {label}."),
            (false, true) => format!("Name the {label} model the assistant should use."),
        }
    }
}

/// A question about a document, as the reader asked it.
pub struct Question<'a> {
    pub prompt: &'a str,
    /// Each page's text.
    pub pages: &'a [String],
    /// The page being read (0-based), kept first when only some pages fit.
    pub focus: Option<usize>,
    /// The question is about the focus page alone, so it fails at once when that page has no
    /// text.
    pub page_only: bool,
}

/// Answers a question about a document as the next turn of `history`: the answer, with a note
/// of the pages it read (when not all of them) and what it cost, and the usage to record.
pub fn answer(
    transport: &dyn Transport,
    setup: &Setup,
    q: &Question,
    history: &[Turn],
) -> Result<(Turn, Usage), String> {
    if let Some(page) = q
        .focus
        .filter(|&p| q.page_only && q.pages.get(p).is_some_and(|t| t.trim().is_empty()))
    {
        return Err(format!(
            "Page {} has no text layer: it is a picture of text. Text recognition (OCR) is not \
             available yet.",
            page + 1
        ));
    }
    let context = Context::new(q.pages, q.prompt, q.focus)?;
    let reply = ask(
        transport,
        &Ask {
            provider: setup.provider,
            key: &setup.key,
            model: &setup.model,
            prompt: q.prompt,
            context: &context,
            history,
        },
    )?;
    let mut note = reply.usage.summary();
    if context.partial() {
        note = format!(
            "Read pages {} of {} · {note}",
            ranges(&context.pages),
            context.page_count
        );
    }
    Ok((Turn::assistant(&reply.text, note), reply.usage))
}

/// One question to the assistant.
pub struct Ask<'a> {
    pub provider: Provider,
    pub key: &'a str,
    pub model: &'a str,
    pub prompt: &'a str,
    pub context: &'a Context,
    pub history: &'a [Turn],
}

pub fn ask(transport: &dyn Transport, q: &Ask) -> Result<Answer, String> {
    // Both a key and a model name are required. Neither is guessed: an unconfigured assistant
    // says so rather than sending a request that would fail, or silently answering as some model
    // the reader did not choose.
    let label = q.provider.label();
    match (q.key.trim().is_empty(), q.model.trim().is_empty()) {
        (true, true) => {
            return Err(format!(
                "{label} is not set up yet. Add an API key and a model name."
            ));
        }
        (true, false) => return Err(format!("No {label} API key set. Add one first.")),
        (false, true) => {
            return Err(format!(
                "No model set for {label}. Name the model you want, for example {}.",
                q.provider.example_model()
            ));
        }
        (false, false) => {}
    }
    if q.prompt.trim().is_empty() {
        return Err("Type a question first.".into());
    }

    let model = q.model.trim();
    let messages = build_messages(trim_history(q.history), q.prompt, q.context);
    match q.provider {
        Provider::Anthropic => anthropic(transport, q.key, model, &messages),
        Provider::OpenAi => openai(transport, q.key, model, &messages),
        Provider::Google => google(transport, q.key, model, &messages),
    }
}

/// Keeps the newest turns that fit the history budget, oldest dropped first. The document is
/// deliberately not counted: it is attached separately and has to survive trimming, or the
/// conversation loses the thing it is about.
fn trim_history(history: &[Turn]) -> &[Turn] {
    let budget = MAX_HISTORY_TOKENS * CHARS_PER_TOKEN;

    let mut used = 0;
    let mut start = history.len();
    for (i, turn) in history.iter().enumerate().rev() {
        used += turn.text.len();
        if used > budget {
            break;
        }
        start = i;
    }

    // The document's own exchange opens the request, so the conversation has to resume on a
    // question. A cut that lands on an answer would put two assistant messages back to back,
    // which the APIs reject.
    if history
        .get(start)
        .is_some_and(|turn| turn.role == "assistant")
    {
        start += 1;
    }

    &history[start..]
}

/// Keeps the conversation and the document apart. The document is a message of its own at the
/// head of the request, never glued onto whatever question happened to come first, so a
/// conversation of any length carries exactly one copy of it and trimming old turns can never
/// take it away. The turns that follow are only what was actually said.
fn build_messages(history: &[Turn], prompt: &str, context: &Context) -> Vec<Msg> {
    let mut messages = vec![
        Msg {
            assistant: false,
            content: with_document(&context.text, &context.note()),
        },
        // The three APIs all want user and assistant messages to alternate, so the document gets
        // an answer before the conversation proper starts.
        Msg {
            assistant: true,
            content: DOCUMENT_ACK.to_string(),
        },
    ];

    messages.extend(history.iter().map(|turn| Msg {
        assistant: turn.role == "assistant",
        content: turn.text.clone(),
    }));

    messages.push(Msg {
        assistant: false,
        content: prompt.to_string(),
    });
    messages
}

fn with_document(document: &str, note: &str) -> String {
    let (doc, truncated) = if document.len() > MAX_DOC_CHARS {
        // Cut on a char boundary so multi-byte content doesn't panic the slice.
        let mut end = MAX_DOC_CHARS;
        while end > 0 && !document.is_char_boundary(end) {
            end -= 1;
        }
        (&document[..end], true)
    } else {
        (document, false)
    };

    let mut message = format!("<document>\n{doc}\n</document>");
    if truncated {
        message.push_str("\n(The document was truncated to fit the request.)");
    }
    if !note.is_empty() {
        message.push('\n');
        message.push_str(note);
    }
    message
}

/* ---------- Anthropic Messages API ---------- */

fn anthropic(
    transport: &dyn Transport,
    api_key: &str,
    model: &str,
    messages: &[Msg],
) -> Result<Answer, String> {
    // max_tokens is required by this API. It budgets thinking and response text together, so it
    // is sized well above any answer we want back.
    let body = json!({
        "model": model,
        "max_tokens": MAX_ANSWER_TOKENS,
        "system": SYSTEM_PROMPT,
        "messages": messages
            .iter()
            .map(|m| json!({
                "role": if m.assistant { "assistant" } else { "user" },
                "content": m.content,
            }))
            .collect::<Vec<_>>(),
    });

    let (status, payload) = transport.post(
        "https://api.anthropic.com/v1/messages",
        &[("x-api-key", api_key), ("anthropic-version", "2023-06-01")],
        &body,
    )?;

    if !(200..300).contains(&status) {
        return Err(describe_error("Anthropic", status, &payload));
    }

    if payload.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        return Err("The model declined to answer this request.".into());
    }

    let text = payload
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    let reported = payload.get("usage");
    let usage = Usage {
        input: count(reported, "input_tokens"),
        output: count(reported, "output_tokens"),
    };

    finish(text, usage)
}

/* ---------- OpenAI Chat Completions ---------- */

// GPT-5 models reason before answering, out of the same budget that has to cover the answer.
// "minimal" keeps that spend near zero, which matters most on a metered free tier. Older models
// have no such field and `gpt-5-chat-latest` does not reason at all, so neither is sent one;
// anything unrecognised is left alone and `openai` retries without this field if the API
// rejects it.
fn reasoning_effort(model: &str) -> Option<Value> {
    let model = model.to_ascii_lowercase();
    (model.starts_with("gpt-5") && !model.contains("chat")).then(|| json!("minimal"))
}

fn openai(
    transport: &dyn Transport,
    api_key: &str,
    model: &str,
    messages: &[Msg],
) -> Result<Answer, String> {
    let mut chat = vec![json!({ "role": "system", "content": SYSTEM_PROMPT })];
    chat.extend(messages.iter().map(|m| {
        json!({
            "role": if m.assistant { "assistant" } else { "user" },
            "content": m.content,
        })
    }));

    // max_completion_tokens, not max_tokens: the older field is rejected outright by reasoning
    // models. It budgets thinking and answer text together.
    let body = |effort: Option<&Value>| {
        let mut body = json!({
            "model": model,
            "max_completion_tokens": MAX_ANSWER_TOKENS,
            "messages": chat.clone(),
        });
        if let Some(effort) = effort {
            body["reasoning_effort"] = effort.clone();
        }
        body
    };

    let auth = format!("Bearer {api_key}");
    let url = "https://api.openai.com/v1/chat/completions";
    let headers = [("authorization", auth.as_str())];

    let effort = reasoning_effort(model);
    let (mut status, mut payload) = transport.post(url, &headers, &body(effort.as_ref()))?;

    // A 400 here is most likely this model not knowing the reasoning field. Losing the tuning is
    // far better than losing the answer.
    if status == 400 && effort.is_some() {
        (status, payload) = transport.post(url, &headers, &body(None))?;
    }

    if !(200..300).contains(&status) {
        return Err(describe_error("OpenAI", status, &payload));
    }

    let choice = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first());

    let text = choice
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let reported = payload.get("usage");
    let usage = Usage {
        input: count(reported, "prompt_tokens"),
        // Reasoning tokens are already counted inside this figure.
        output: count(reported, "completion_tokens"),
    };

    // A capped reasoning model can spend the whole budget thinking and stop before writing
    // anything. "Empty response" would hide why.
    if text.trim().is_empty()
        && choice
            .and_then(|c| c.get("finish_reason"))
            .and_then(Value::as_str)
            == Some("length")
    {
        return Err(OUT_OF_BUDGET.into());
    }

    finish(text, usage)
}

const OUT_OF_BUDGET: &str = "The model used its whole output budget on reasoning and never \
started the answer. Ask something shorter, or name a model that reasons less.";

/* ---------- Google AI Studio (Gemini API) ---------- */

// Gemini reasons before it answers, and the control for that was renamed between model
// generations. Anything unrecognised is left alone and gets the model's own default; `google`
// retries without this block if the API rejects it.
fn thinking_config(model: &str) -> Option<Value> {
    let model = model.to_ascii_lowercase();

    if model.starts_with("gemini-3") {
        // Gemini 3 replaced the numeric budget with a level. "low" still reasons, it just cannot
        // swallow the entire output budget doing it.
        Some(json!({ "thinkingLevel": "low" }))
    } else if model.starts_with("gemini-2.5-pro") {
        // 2.5 Pro is the one model that cannot switch thinking off. 128 is its floor.
        Some(json!({ "thinkingBudget": 128 }))
    } else if model.starts_with("gemini-2.5") {
        Some(json!({ "thinkingBudget": 0 }))
    } else {
        None
    }
}

fn google(
    transport: &dyn Transport,
    api_key: &str,
    model: &str,
    messages: &[Msg],
) -> Result<Answer, String> {
    // The key goes in a header rather than the documented ?key= query parameter so it never
    // lands in a URL that could be logged.
    let url =
        format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent");
    let headers = [("x-goog-api-key", api_key)];

    let contents = messages
        .iter()
        .map(|m| {
            json!({
                "role": if m.assistant { "model" } else { "user" },
                "parts": [{ "text": m.content }],
            })
        })
        .collect::<Vec<_>>();

    let body = |thinking: Option<&Value>| {
        let mut generation = json!({ "maxOutputTokens": MAX_ANSWER_TOKENS });
        if let Some(thinking) = thinking {
            generation["thinkingConfig"] = thinking.clone();
        }
        json!({
            "system_instruction": { "parts": [{ "text": SYSTEM_PROMPT }] },
            "contents": contents.clone(),
            "generationConfig": generation,
        })
    };

    let thinking = thinking_config(model);
    let (mut status, mut payload) = transport.post(&url, &headers, &body(thinking.as_ref()))?;

    // A 400 here is most likely this model not knowing the thinking field. Losing the tuning is
    // far better than losing the answer.
    if status == 400 && thinking.is_some() {
        (status, payload) = transport.post(&url, &headers, &body(None))?;
    }

    if !(200..300).contains(&status) {
        return Err(describe_error("Google AI Studio", status, &payload));
    }

    if let Some(reason) = payload
        .get("promptFeedback")
        .and_then(|f| f.get("blockReason"))
        .and_then(Value::as_str)
    {
        return Err(format!("The request was blocked by the model ({reason})."));
    }

    let candidate = payload
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|c| c.first());

    let reported = payload.get("usageMetadata");
    let usage = Usage {
        input: count(reported, "promptTokenCount"),
        // Reasoning is counted separately here, unlike the other two providers, so it has to be
        // added back in to be comparable.
        output: count(reported, "candidatesTokenCount") + count(reported, "thoughtsTokenCount"),
    };

    let text = candidate
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                // A reasoning part is the model's scratch work, not its answer.
                .filter(|p| p.get("thought").and_then(Value::as_bool) != Some(true))
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    if text.trim().is_empty() {
        // A Gemini call can succeed and still carry no answer. Saying "empty response" hides
        // which of several very different things went wrong.
        let reason = candidate
            .and_then(|c| c.get("finishReason"))
            .and_then(Value::as_str)
            .unwrap_or("none given");

        return Err(match reason {
            "MAX_TOKENS" => OUT_OF_BUDGET.into(),
            "SAFETY" | "PROHIBITED_CONTENT" => {
                "The model stopped for a safety reason and returned no answer.".into()
            }
            "RECITATION" => {
                "The model stopped because the answer reproduced its training data.".into()
            }
            other => format!("Google AI Studio returned no answer text (finish reason: {other})."),
        });
    }

    Ok(Answer { text, usage })
}

/* ---------- Shared plumbing ---------- */

fn finish(text: String, usage: Usage) -> Result<Answer, String> {
    if text.trim().is_empty() {
        Err("The model returned an empty response. Try rewording the question.".into())
    } else {
        Ok(Answer { text, usage })
    }
}

fn describe_error(provider: &str, status: u16, payload: &Value) -> String {
    let detail = payload
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("no further detail");

    match status {
        400 => format!("{provider} rejected the request: {detail}"),
        401 => format!(
            "That {provider} API key was not accepted. Check it in the assistant's settings."
        ),
        403 => format!("That {provider} API key does not have access to this model."),
        404 => format!(
            "{provider} has no model by that name. Check the model in the assistant's settings."
        ),
        413 => "The document is too large to send.".into(),
        429 => format!("{provider} is rate limiting these requests. Wait a moment and try again."),
        500..=599 => format!("{provider} is unavailable right now. Try again shortly."),
        _ => format!("{provider} returned {status}: {detail}"),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// A request as the stand-in provider received it: URL, headers, body.
    type Sent = (String, Vec<(String, String)>, Value);

    /// A stand-in provider: replies in turn, and keeps every request it was sent.
    struct Mock {
        replies: RefCell<Vec<(u16, Value)>>,
        sent: RefCell<Vec<Sent>>,
    }

    impl Mock {
        fn new(replies: Vec<(u16, Value)>) -> Mock {
            Mock {
                replies: RefCell::new(replies),
                sent: RefCell::default(),
            }
        }

        fn header(&self, call: usize, name: &str) -> Option<String> {
            self.sent.borrow()[call]
                .1
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
        }
    }

    impl Transport for Mock {
        fn post(
            &self,
            url: &str,
            headers: &[(&str, &str)],
            body: &Value,
        ) -> Result<(u16, Value), String> {
            let headers = headers
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect();
            self.sent
                .borrow_mut()
                .push((url.into(), headers, body.clone()));
            Ok(self.replies.borrow_mut().remove(0))
        }
    }

    fn turn(role: &str, text: &str) -> Turn {
        Turn {
            role: role.into(),
            text: text.into(),
            note: String::new(),
        }
    }

    fn doc(text: &str) -> Context {
        Context::new(&[text.to_string()], "", None).unwrap()
    }

    fn question<'a>(
        provider: Provider,
        model: &'a str,
        context: &'a Context,
        history: &'a [Turn],
    ) -> Ask<'a> {
        Ask {
            provider,
            key: "sk-test",
            model,
            prompt: "What is it about?",
            context,
            history,
        }
    }

    #[test]
    fn anthropic_requests_and_answers() {
        let mock = Mock::new(vec![(
            200,
            json!({
                "content": [{"type": "thinking", "thinking": "hm"}, {"type": "text", "text": "Cats "}, {"type": "text", "text": "[p. 1]"}],
                "usage": {"input_tokens": 120, "output_tokens": 8},
            }),
        )]);
        let context = doc("All about cats.");
        let history = [turn("user", "hi"), turn("assistant", "hello")];
        let answer = ask(
            &mock,
            &question(Provider::Anthropic, " claude-x ", &context, &history),
        )
        .unwrap();
        assert_eq!(answer.text, "Cats [p. 1]");
        assert_eq!(
            answer.usage,
            Usage {
                input: 120,
                output: 8
            }
        );

        let sent = mock.sent.borrow();
        let (url, _, body) = &sent[0];
        assert_eq!(url, "https://api.anthropic.com/v1/messages");
        assert_eq!(mock.header(0, "x-api-key").as_deref(), Some("sk-test"));
        assert_eq!(
            mock.header(0, "anthropic-version").as_deref(),
            Some("2023-06-01")
        );
        assert_eq!(body["model"], "claude-x");
        assert_eq!(body["max_tokens"], MAX_ANSWER_TOKENS);
        assert!(body["system"].as_str().unwrap().contains("[p. 12]"));
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 5);
        assert!(
            messages[0]["content"]
                .as_str()
                .unwrap()
                .contains("[p. 1]\nAll about cats.")
        );
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[4]["content"], "What is it about?");
    }

    #[test]
    fn anthropic_refusals_and_errors_are_explained() {
        let context = doc("x");
        let mock = Mock::new(vec![
            (200, json!({"stop_reason": "refusal", "content": []})),
            (401, json!({"error": {"message": "invalid x-api-key"}})),
            (400, json!({"error": {"message": "prompt is too long"}})),
        ]);
        let q = question(Provider::Anthropic, "claude-x", &context, &[]);
        assert!(ask(&mock, &q).unwrap_err().contains("declined"));
        assert!(ask(&mock, &q).unwrap_err().contains("key was not accepted"));
        assert_eq!(
            ask(&mock, &q).unwrap_err(),
            "Anthropic rejected the request: prompt is too long"
        );
    }

    #[test]
    fn openai_requests_and_answers() {
        let mock = Mock::new(vec![(
            200,
            json!({
                "choices": [{"message": {"content": "Dogs [p. 1]"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 90, "completion_tokens": 30},
            }),
        )]);
        let context = doc("Dogs.");
        let answer = ask(&mock, &question(Provider::OpenAi, "gpt-4.1", &context, &[])).unwrap();
        assert_eq!(answer.text, "Dogs [p. 1]");
        assert_eq!(
            answer.usage,
            Usage {
                input: 90,
                output: 30
            }
        );

        let sent = mock.sent.borrow();
        let (url, _, body) = &sent[0];
        assert_eq!(url, "https://api.openai.com/v1/chat/completions");
        assert_eq!(
            mock.header(0, "authorization").as_deref(),
            Some("Bearer sk-test")
        );
        assert_eq!(body["max_completion_tokens"], MAX_ANSWER_TOKENS);
        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][2]["role"], "assistant");
    }

    #[test]
    fn openai_retries_without_the_reasoning_field() {
        let mock = Mock::new(vec![
            (
                400,
                json!({"error": {"message": "Unsupported parameter: 'reasoning_effort'"}}),
            ),
            (200, json!({"choices": [{"message": {"content": "ok"}}]})),
        ]);
        let context = doc("x");
        assert_eq!(
            ask(
                &mock,
                &question(Provider::OpenAi, "gpt-5.6-luna", &context, &[])
            )
            .unwrap()
            .text,
            "ok"
        );
        let sent = mock.sent.borrow();
        assert_eq!(sent[0].2["reasoning_effort"], "minimal");
        assert!(sent[1].2.get("reasoning_effort").is_none());
    }

    #[test]
    fn openai_says_when_reasoning_used_the_whole_budget() {
        let mock = Mock::new(vec![(
            200,
            json!({"choices": [{"message": {"content": ""}, "finish_reason": "length"}]}),
        )]);
        let context = doc("x");
        let err = ask(&mock, &question(Provider::OpenAi, "gpt-4.1", &context, &[])).unwrap_err();
        assert!(err.contains("output budget"), "got: {err}");
    }

    #[test]
    fn google_requests_and_answers() {
        let mock = Mock::new(vec![(
            200,
            json!({
                "candidates": [{"content": {"parts": [{"text": "plan", "thought": true}, {"text": "Birds [p. 1]"}]}}],
                "usageMetadata": {"promptTokenCount": 50, "candidatesTokenCount": 7, "thoughtsTokenCount": 13},
            }),
        )]);
        let context = doc("Birds.");
        let history = [turn("user", "hi"), turn("assistant", "hello")];
        let answer = ask(
            &mock,
            &question(Provider::Google, "gemini-3.7-flash", &context, &history),
        )
        .unwrap();
        assert_eq!(answer.text, "Birds [p. 1]");
        assert_eq!(
            answer.usage,
            Usage {
                input: 50,
                output: 20
            }
        );

        let sent = mock.sent.borrow();
        let (url, _, body) = &sent[0];
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.7-flash:generateContent"
        );
        assert!(!url.contains("key="));
        assert_eq!(mock.header(0, "x-goog-api-key").as_deref(), Some("sk-test"));
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "low"
        );
        assert_eq!(body["contents"][1]["role"], "model");
        assert_eq!(body["contents"][3]["role"], "model");
        assert!(
            body["system_instruction"]["parts"][0]["text"]
                .as_str()
                .unwrap()
                .contains("PDF")
        );
    }

    #[test]
    fn google_retries_without_thinking_and_explains_empty_answers() {
        let context = doc("x");
        let mock = Mock::new(vec![
            (
                400,
                json!({"error": {"message": "thinking is not supported"}}),
            ),
            (
                200,
                json!({"candidates": [{"content": {"parts": []}, "finishReason": "MAX_TOKENS"}]}),
            ),
            (200, json!({"promptFeedback": {"blockReason": "SAFETY"}})),
            (200, json!({"candidates": [{"finishReason": "RECITATION"}]})),
        ]);
        let q = question(Provider::Google, "gemini-2.5-flash", &context, &[]);
        assert!(ask(&mock, &q).unwrap_err().contains("output budget"));
        assert!(
            mock.sent.borrow()[1].2["generationConfig"]
                .get("thinkingConfig")
                .is_none()
        );
        assert_eq!(
            ask(&mock, &q).unwrap_err(),
            "The request was blocked by the model (SAFETY)."
        );
        assert!(ask(&mock, &q).unwrap_err().contains("training data"));
    }

    #[test]
    fn an_unconfigured_assistant_says_what_is_missing() {
        let mock = Mock::new(vec![]);
        let context = doc("x");
        let mut q = question(Provider::Google, "", &context, &[]);
        q.key = " ";
        assert!(ask(&mock, &q).unwrap_err().contains("not set up yet"));
        q.model = "gemini-3.7-flash";
        assert!(
            ask(&mock, &q)
                .unwrap_err()
                .starts_with("No Google AI Studio API key")
        );
        q.key = "k";
        q.model = "";
        assert!(ask(&mock, &q).unwrap_err().contains("gemini-3.7-flash"));
        q.model = "m";
        q.prompt = "  ";
        assert_eq!(ask(&mock, &q).unwrap_err(), "Type a question first.");
        assert!(mock.sent.borrow().is_empty());
    }

    #[test]
    fn reasoning_effort_targets_the_gpt5_family_only() {
        assert!(reasoning_effort("gpt-5.6-luna").is_some());
        assert!(reasoning_effort("gpt-5.4-mini").is_some());
        assert!(reasoning_effort("GPT-5").is_some());
        // Non-reasoning and older models must not be sent the field.
        assert!(reasoning_effort("gpt-5-chat-latest").is_none());
        assert!(reasoning_effort("gpt-4.1").is_none());
        assert!(reasoning_effort("o3").is_none());
    }

    #[test]
    fn thinking_config_follows_the_model_generation() {
        assert_eq!(
            thinking_config("gemini-3.7-flash").unwrap()["thinkingLevel"],
            "low"
        );
        assert_eq!(
            thinking_config("gemini-2.5-pro").unwrap()["thinkingBudget"],
            128
        );
        assert_eq!(
            thinking_config("gemini-2.5-flash").unwrap()["thinkingBudget"],
            0
        );
        assert!(thinking_config("gemini-2.0-flash").is_none());
    }

    #[test]
    fn oversized_documents_are_truncated_on_a_char_boundary() {
        // A multi-byte character straddling the cut must not panic the slice.
        let doc = "é".repeat(MAX_DOC_CHARS);
        assert!(with_document(&doc, "").contains("truncated"));
    }

    #[test]
    fn the_document_is_a_message_of_its_own() {
        let history = [turn("user", "what is this?"), turn("assistant", "A title.")];
        let messages = build_messages(&history, "and now?", &doc("Title"));

        // The document stands alone; the first question is not glued to it.
        assert!(messages[0].content.starts_with("<document>"));
        assert!(messages[0].content.ends_with("</document>"));
        assert!(!messages[0].content.contains("what is this?"));
        assert_eq!(messages[1].content, DOCUMENT_ACK);
        assert_eq!(messages[2].content, "what is this?");
        assert_eq!(messages[4].content, "and now?");
    }

    #[test]
    fn a_long_conversation_carries_one_copy_of_the_document() {
        let history: Vec<Turn> = (0..20)
            .map(|i| {
                turn(
                    if i % 2 == 0 { "user" } else { "assistant" },
                    "said something",
                )
            })
            .collect();
        let messages = build_messages(&history, "and now?", &doc("Title"));
        let copies = messages
            .iter()
            .filter(|m| m.content.contains("<document>"))
            .count();
        assert_eq!(copies, 1);
    }

    #[test]
    fn roles_alternate_across_the_whole_request() {
        let history = [turn("user", "one"), turn("assistant", "two")];
        let messages = build_messages(&history, "three", &doc("Title"));
        for pair in messages.windows(2) {
            assert_ne!(pair[0].assistant, pair[1].assistant);
        }
    }

    // A chat is only ever appended to in question-and-answer pairs, so a real history alternates
    // and ends on an answer. These fixtures follow that.
    #[test]
    fn history_is_trimmed_from_the_oldest_end() {
        let budget = MAX_HISTORY_TOKENS * CHARS_PER_TOKEN;
        let history = [
            turn("user", &"a".repeat(budget)),
            turn("assistant", "old answer"),
            turn("user", "recent question"),
            turn("assistant", "recent answer"),
        ];
        let kept = trim_history(&history);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].text, "recent question");
    }

    #[test]
    fn trimming_resumes_on_a_question_not_an_answer() {
        let budget = MAX_HISTORY_TOKENS * CHARS_PER_TOKEN;
        // The cut lands mid-exchange, on an answer whose question no longer fits.
        let history = [
            turn("user", &"a".repeat(budget)),
            turn("assistant", "orphaned answer"),
            turn("user", "question"),
            turn("assistant", "reply"),
        ];
        let kept = trim_history(&history);
        assert_eq!(kept[0].role, "user");
        assert_eq!(kept[0].text, "question");

        let messages = build_messages(kept, "and now?", &doc("Title"));
        for pair in messages.windows(2) {
            assert_ne!(pair[0].assistant, pair[1].assistant);
        }
    }

    #[test]
    fn trimming_never_reaches_the_document() {
        let budget = MAX_HISTORY_TOKENS * CHARS_PER_TOKEN;
        let history = [turn("user", &"a".repeat(budget + 1))];
        let messages = build_messages(trim_history(&history), "and now?", &doc("Title"));
        assert!(messages[0].content.contains("Title"));
    }

    #[test]
    fn answers_carry_a_note_of_pages_read_and_cost() {
        let mock = Mock::new(vec![(
            200,
            json!({"content": [{"type": "text", "text": "Yes [p. 2]"}], "usage": {"input_tokens": 1_200, "output_tokens": 30}}),
        )]);
        let setup = Setup {
            provider: Provider::Anthropic,
            model: "m".into(),
            key: "k".into(),
        };
        let pages = vec!["one".to_string(), "two".to_string()];
        let q = Question {
            prompt: "Is it?",
            pages: &pages,
            focus: None,
            page_only: false,
        };
        let (turn, usage) = answer(&mock, &setup, &q, &[]).unwrap();
        assert_eq!(
            turn,
            Turn::assistant("Yes [p. 2]", "1.2k in · 30 out".into())
        );
        assert_eq!(
            usage,
            Usage {
                input: 1_200,
                output: 30
            }
        );

        let pages = vec!["one".to_string(), " ".to_string()];
        let q = Question {
            prompt: "Summarize page 2.",
            pages: &pages,
            focus: Some(1),
            page_only: true,
        };
        assert!(
            answer(&mock, &setup, &q, &[])
                .unwrap_err()
                .starts_with("Page 2 has no text layer")
        );
    }

    #[test]
    fn the_setup_says_what_is_missing() {
        let mut setup = Setup {
            provider: Provider::OpenAi,
            ..Setup::default()
        };
        assert_eq!(setup.label(), "OpenAI");
        assert!(setup.notice().contains("API key and a model name"));
        setup.model = "gpt-5.6-luna".into();
        assert_eq!(setup.label(), "OpenAI · gpt-5.6-luna");
        assert_eq!(setup.notice(), "The assistant needs an API key for OpenAI.");
        setup.key = "k".into();
        assert_eq!(setup.notice(), "");
    }

    #[test]
    fn turns_keep_their_note_out_of_the_request() {
        let history = [turn("user", "q"), Turn::assistant("a", "Pages 1–2".into())];
        let messages = build_messages(&history, "next", &doc("Title"));
        assert!(messages.iter().all(|m| !m.content.contains("Pages 1–2")));
        let kept: Vec<Turn> =
            serde_json::from_str(&serde_json::to_string(&history).unwrap()).unwrap();
        assert_eq!(kept, history);
        assert!(!serde_json::to_string(&history[0]).unwrap().contains("note"));
    }
}
