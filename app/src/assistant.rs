//! The assistant panel (Ctrl+Shift+A): questions about the active document, answered through
//! mp-ai on a worker thread. Each document keeps one chat, stored by content hash so it survives
//! renames; answers cite pages, and a citation goes to its page.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;

use mp_ai::{Config, Provider, Question, Setup, Turn, WinHttp};
use mp_engine::{DocId, Engine};
use slint::{ComponentHandle, ModelRc, StyledText, VecModel};

use crate::viewer::{self, App};
use crate::{Assistant, ChatItem, MainWindow};

/// The document the panel shows a chat for.
struct Doc {
    id: DocId,
    /// Known once the file has been read; empty when it could not be, and then the chat is not
    /// kept.
    hash: Option<String>,
    /// Each page's text, kept after the first question.
    pages: Option<Arc<Vec<String>>>,
}

#[derive(Default)]
struct State {
    window: slint::Weak<MainWindow>,
    doc: Option<Doc>,
    chat: Vec<Turn>,
    /// The question out, by number, and its document. One at a time; an answer whose number is
    /// no longer here (a new chat was started) is dropped.
    pending: Option<(u64, DocId)>,
    asked: u64,
    error: Option<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

fn ui() -> Option<MainWindow> {
    STATE.with_borrow(|s| s.window.upgrade())
}

pub fn wire(window: &MainWindow) {
    STATE.with_borrow_mut(|s| s.window = window.as_weak());
    let panel = window.global::<Assistant>();
    panel.on_send(|| {
        viewer::with(ask_draft);
    });
    panel.on_quick(|what| {
        viewer::with(|app| quick(app, &what));
    });
    panel.on_new_chat(new_chat);
    panel.on_close(|| {
        viewer::with(toggle);
    });
    panel.on_link(|link| {
        if let Some(page) = link
            .strip_prefix("page:")
            .and_then(|p| p.parse::<usize>().ok())
        {
            viewer::with(|app| app.flash_page(page.saturating_sub(1)));
        } else {
            viewer::with(|app| app.confirm_uri(link.to_string()));
        }
    });
    panel.on_copy(|i| copy(i as usize));
    panel.on_provider_picked(|i| setup_form(Provider::ALL[i as usize % 3]));
    panel.on_save_setup(save_setup);
    panel.on_remove_key(|| {
        let Some(window) = ui() else { return };
        let provider = Provider::ALL[window.global::<Assistant>().get_provider() as usize % 3];
        let _ = mp_ai::set_key(provider, "");
        setup_form(provider);
        show();
    });
}

pub fn toggle(app: &mut App) {
    let Some(window) = ui() else { return };
    let panel = window.global::<Assistant>();
    let open = !panel.get_open();
    panel.set_open(open);
    if open {
        setup_form(Config::load().provider);
        follow(app);
        show();
        window.invoke_focus_assistant();
    } else {
        window.invoke_focus_view();
    }
}

/// Shows the chat of the active document, once the panel is open.
pub fn follow(app: &App) {
    let Some(window) = ui() else { return };
    if !window.global::<Assistant>().get_open() {
        return;
    }
    let reading = app.reading();
    let id = reading.as_ref().map(|r| r.0);
    if STATE.with_borrow(|s| s.doc.as_ref().map(|d| d.id) == id) {
        return;
    }
    STATE.with_borrow_mut(|s| {
        s.chat.clear();
        s.error = None;
        s.doc = id.map(|id| Doc {
            id,
            hash: None,
            pages: None,
        });
    });
    show();
    if let Some((id, path, ..)) = reading {
        std::thread::spawn(move || read_chat(id, path));
    }
}

fn read_chat(id: DocId, path: PathBuf) {
    let hash = std::fs::read(&path)
        .map(|bytes| mp_ai::content_hash(&bytes))
        .unwrap_or_default();
    let chat = mp_ai::load_chat(&hash);
    let _ = slint::invoke_from_event_loop(move || {
        STATE.with_borrow_mut(|s| {
            if let Some(doc) = s.doc.as_mut().filter(|d| d.id == id) {
                doc.hash = Some(hash);
                s.chat = chat;
            }
        });
        show();
    });
}

fn new_chat() {
    STATE.with_borrow_mut(|s| {
        let id = s.doc.as_ref().map(|d| d.id);
        if s.pending.is_some_and(|(_, doc)| Some(doc) == id) {
            s.pending = None;
        }
        s.chat.clear();
        s.error = None;
        if let Some(hash) = s.doc.as_ref().and_then(|d| d.hash.as_deref()) {
            mp_ai::save_chat(hash, &[]);
        }
    });
    show();
}

fn ask_draft(app: &mut App) {
    let Some(window) = ui() else { return };
    let panel = window.global::<Assistant>();
    let prompt = panel.get_draft().trim().to_string();
    if !prompt.is_empty() && ask(app, prompt, None, false) {
        panel.set_draft("".into());
    }
}

/// The quick actions: summarize the document or the page being read, explain the selection.
pub fn quick(app: &mut App, what: &str) {
    if ui().is_some_and(|w| !w.global::<Assistant>().get_open()) {
        toggle(app);
    }
    let Some((_, _, current, _)) = app.reading() else {
        return fail("Open a PDF first.");
    };
    match what {
        "document" => {
            let prompt = "Summarize this document in a few short paragraphs or bullet points.";
            ask(app, prompt.into(), None, false);
        }
        "page" => {
            ask(
                app,
                format!("Summarize page {}.", current + 1),
                Some(current),
                true,
            );
        }
        _ => match app.selected_text().filter(|t| !t.trim().is_empty()) {
            Some(text) => {
                let prompt = format!("Explain this passage:\n\n{}", text.trim());
                ask(app, prompt, Some(current), false);
            }
            None => fail("Select some text on a page first."),
        },
    }
}

fn fail(error: &str) {
    STATE.with_borrow_mut(|s| s.error = Some(error.into()));
    show();
}

/// Sends a question about the active document; false when one is already out or the document is
/// still being read. `focus` is the page being read, kept first when only some pages fit;
/// `page_only` asks about that page alone.
fn ask(app: &mut App, prompt: String, focus: Option<usize>, page_only: bool) -> bool {
    let Some((id, _, _, count)) = app.reading() else {
        fail("Open a PDF first.");
        return false;
    };
    let ready = STATE.with_borrow(|s| {
        s.pending.is_none()
            && s.doc
                .as_ref()
                .is_some_and(|d| d.id == id && d.hash.is_some())
    });
    if !ready {
        return false;
    }
    let setup = Setup::load();
    let engine = app.engine();
    let (number, history, hash, pages) = STATE.with_borrow_mut(|s| {
        s.asked += 1;
        s.pending = Some((s.asked, id));
        s.error = None;
        let history = s.chat.clone();
        s.chat.push(Turn::user(&prompt));
        let doc = s.doc.as_ref().expect("checked above");
        (
            s.asked,
            history,
            doc.hash.clone().unwrap_or_default(),
            doc.pages.clone(),
        )
    });
    show();

    std::thread::spawn(move || {
        let pages = pages.unwrap_or_else(|| Arc::new(read_pages(&engine, id, count)));
        let question = Question {
            prompt: &prompt,
            pages: &pages,
            focus,
            page_only,
        };
        let result = mp_ai::answer(&WinHttp, &setup, &question, &history).map(|(turn, usage)| {
            Config::record(setup.provider, usage);
            turn
        });
        let _ = slint::invoke_from_event_loop(move || {
            answered(number, id, hash, history, prompt, pages, result)
        });
    });
    true
}

fn read_pages(engine: &Engine, doc: DocId, count: usize) -> Vec<String> {
    (0..count)
        .map(|page| {
            engine
                .display_list(doc, page)
                .and_then(|list| mp_engine::page_text(&list))
                .map(|t| t.text(0..t.chars.len()))
                .unwrap_or_default()
        })
        .collect()
}

fn answered(
    number: u64,
    id: DocId,
    hash: String,
    mut chat: Vec<Turn>,
    prompt: String,
    pages: Arc<Vec<String>>,
    result: Result<Turn, String>,
) {
    let (current, showing) = STATE.with_borrow_mut(|s| {
        let showing = s
            .doc
            .as_mut()
            .filter(|d| d.id == id)
            .map(|d| d.pages.get_or_insert(pages))
            .is_some();
        let current = s.pending.is_some_and(|(n, _)| n == number);
        if current {
            s.pending = None;
        }
        (current, showing)
    });
    if !current {
        return;
    }
    match result {
        Ok(answer) => {
            chat.push(Turn::user(&prompt));
            chat.push(answer);
            mp_ai::save_chat(&hash, &chat);
            if showing {
                STATE.with_borrow_mut(|s| s.chat = chat);
            }
        }
        Err(error) if showing => {
            STATE.with_borrow_mut(|s| {
                s.chat = chat;
                s.error = Some(error);
            });
            // The question goes back in the box, to fix or send again.
            if let Some(window) = ui() {
                let panel = window.global::<Assistant>();
                if panel.get_draft().is_empty() {
                    panel.set_draft(prompt.into());
                }
            }
        }
        Err(_) => {}
    }
    show();
}

/// Fills the setup form for `provider`.
fn setup_form(provider: Provider) {
    let Some(window) = ui() else { return };
    let panel = window.global::<Assistant>();
    let config = Config::load();
    panel.set_provider(
        Provider::ALL
            .iter()
            .position(|&p| p == provider)
            .unwrap_or(0) as i32,
    );
    panel.set_setup_model(config.model(provider).into());
    panel.set_setup_key("".into());
    panel.set_has_key(mp_ai::key(provider).is_some());
    panel.set_example_model(provider.example_model().into());
}

fn save_setup() {
    let Some(window) = ui() else { return };
    let panel = window.global::<Assistant>();
    let provider = Provider::ALL[panel.get_provider() as usize % 3];
    let mut config = Config::load();
    config.provider = provider;
    config
        .models
        .insert(provider, panel.get_setup_model().trim().to_string());
    config.save();
    let key = panel.get_setup_key();
    if let Err(e) = mp_ai::set_key(provider, &key) {
        return fail(&e);
    }
    setup_form(provider);
    panel.set_setup(false);
    show();
}

fn copy(index: usize) {
    let text = STATE.with_borrow(|s| s.chat.get(index).map(|t| t.text.clone()));
    if let Some(text) = text {
        let result = arboard::Clipboard::new().and_then(|mut c| c.set_text(text));
        viewer::with(|app| {
            app.status(
                if result.is_ok() {
                    "Copied the answer"
                } else {
                    "Could not copy"
                }
                .into(),
            )
        });
    }
}

/// Puts the state on the panel.
fn show() {
    let Some(window) = ui() else { return };
    let panel = window.global::<Assistant>();
    STATE.with_borrow(|s| {
        let mut items: Vec<ChatItem> = s.chat.iter().map(item).collect();
        if let Some(error) = &s.error {
            items.push(ChatItem {
                kind: 2,
                text: error.into(),
                ..ChatItem::default()
            });
        }
        panel.set_messages(ModelRc::new(VecModel::from(items)));
        let busy = match &s.doc {
            Some(d) if d.hash.is_none() => "Reading the document…",
            Some(d) if s.pending.is_some_and(|(_, id)| id == d.id) => "Thinking…",
            _ => "",
        };
        panel.set_busy(busy.into());
    });

    let setup = Setup::load();
    panel.set_model(setup.label().into());
    panel.set_notice(setup.notice().into());
    panel.set_usage(Config::load().today(setup.provider).summary().into());
}

fn item(turn: &Turn) -> ChatItem {
    let answer = turn.role == "assistant";
    ChatItem {
        kind: answer as i32,
        text: turn.text.as_str().into(),
        styled: if answer {
            styled(&turn.text)
        } else {
            StyledText::default()
        },
        note: turn.note.as_str().into(),
    }
}

/// The answer as styled text, citations as page links; plain text when it will not parse.
fn styled(markdown: &str) -> StyledText {
    StyledText::from_markdown(&mp_ai::link_citations(&simplify(markdown)))
        .unwrap_or_else(|_| StyledText::from_plain_text(markdown))
}

/// Rewrites what StyledText cannot show (it rejects headings, quotes, rules, tables, code blocks
/// and HTML): headings become bold lines, quotes and table rows plain lines, code escaped text;
/// rules and table borders go.
fn simplify(markdown: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut fenced = false;
    for line in markdown.lines() {
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            out.push(format!("{}  ", escape(t)));
            continue;
        }
        let bare: Vec<char> = t.chars().filter(|&c| c != ' ').collect();
        let rule =
            bare.len() >= 3 && "-*_=".contains(bare[0]) && bare.iter().all(|&c| c == bare[0]);
        let border = t.starts_with('|') && t.chars().all(|c| "|-: ".contains(c));
        if rule || border {
            continue;
        }
        let level = t.chars().take_while(|&c| c == '#').count();
        let line = if (1..=6).contains(&level) && t[level..].starts_with(' ') {
            format!("**{}**", t[level..].trim().trim_end_matches('#').trim())
        } else if let Some(quote) = t.strip_prefix('>') {
            quote.trim_start_matches('>').trim().to_string()
        } else if t.starts_with('|') {
            t.trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect::<Vec<_>>()
                .join(" · ")
        } else {
            line.trim_end().to_string()
        };
        out.push(escape_html(&line));
    }
    out.join("\n")
}

/// Backslash-escapes every ASCII punctuation mark, so code reads as plain text.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Escapes `<` outside code spans, so it is never taken for HTML.
fn escape_html(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut code = false;
    for c in line.chars() {
        match c {
            '`' => code = !code,
            '<' if !code => out.push('\\'),
            _ => {}
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_simplified_to_what_styled_text_shows() {
        let answer = "# Summary\n\nThe report [p. 2] finds:\n\n- costs rose\n- 3 < 4\n\n---\n\n> quoted\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\nSee <https://example.com>.";
        let simple = simplify(answer);
        assert!(simple.starts_with("**Summary**\n"));
        assert!(simple.contains("- 3 \\< 4"));
        assert!(!simple.contains("---"));
        assert!(simple.contains("\nquoted\n"));
        assert!(simple.contains("a · b\n1 · 2"));
        assert!(simple.contains("fn main\\(\\) \\{\\}  "));
        for text in [
            answer,
            "Plain.",
            "**bold** and *it* and `x < y`",
            "1. one\n2. two",
        ] {
            assert!(
                StyledText::from_markdown(&mp_ai::link_citations(&simplify(text))).is_ok(),
                "did not parse: {text}"
            );
        }
    }

    #[test]
    fn citations_become_links() {
        let linked = mp_ai::link_citations(&simplify("Costs rose [p. 12]."));
        assert_eq!(linked, "Costs rose [p. 12](page:12).");
    }
}
