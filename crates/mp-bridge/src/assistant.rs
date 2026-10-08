//! The assistant for the extension. The viewer sends the document's page text and the question;
//! the bridge asks the provider set up in micropdf, so the API key never leaves Credential Manager
//! and this process. Chats are the app's own, found by the document's content hash.
//!
//! `assistant` → the setup and today's usage; `chat {hash}` → the document's chat; `ask {hash,
//! prompt, pages, focus?, pageOnly?}` → the answer, kept in the chat; `new-chat {hash}` → an
//! empty chat.

use mp_ai::{Config, Question, Setup, Transport, Turn};
use serde_json::{Value, json};

/// Native messaging caps what a host sends at 1 MB; a long chat sends its newest turns.
const MAX_REPLY: usize = 900_000;

/// The reply to an assistant message, or None when `message` is not one.
pub fn reply(
    message: &Value,
    transport: &dyn Transport,
    setup: impl FnOnce() -> Setup,
) -> Option<Value> {
    let hash = message["hash"].as_str().unwrap_or_default();
    Some(match message["type"].as_str()? {
        "assistant" => status(&setup()),
        "chat" => chat(&mp_ai::load_chat(hash)),
        "new-chat" => {
            mp_ai::save_chat(hash, &[]);
            chat(&[])
        }
        "ask" => ask(message, hash, transport, &setup()),
        _ => return None,
    })
}

fn status(setup: &Setup) -> Value {
    json!({
        "type": "assistant",
        "model": setup.label(),
        "notice": setup.notice(),
        "usage": Config::load().today(setup.provider).summary(),
    })
}

fn chat(turns: &[Turn]) -> Value {
    let mut start = 0;
    while start < turns.len()
        && serde_json::to_string(&turns[start..]).map_or(0, |s| s.len()) > MAX_REPLY
    {
        // Whole exchanges go, so the chat still opens on a question.
        start += 2;
    }
    json!({"type": "chat", "turns": turns.get(start..).unwrap_or_default()})
}

fn ask(message: &Value, hash: &str, transport: &dyn Transport, setup: &Setup) -> Value {
    let pages: Vec<String> = serde_json::from_value(message["pages"].clone()).unwrap_or_default();
    let prompt = message["prompt"].as_str().unwrap_or_default();
    let question = Question {
        prompt,
        pages: &pages,
        focus: message["focus"].as_u64().map(|p| p as usize),
        page_only: message["pageOnly"].as_bool().unwrap_or(false),
    };
    let mut turns = mp_ai::load_chat(hash);
    match mp_ai::answer(transport, setup, &question, &turns) {
        Ok((turn, usage)) => {
            let today = Config::record(setup.provider, usage);
            turns.push(Turn::user(prompt));
            turns.push(turn.clone());
            mp_ai::save_chat(hash, &turns);
            json!({"type": "answer", "turn": turn, "usage": today.summary()})
        }
        Err(e) => json!({"type": "error", "message": e}),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use mp_ai::Provider;

    use super::*;

    struct Mock(RefCell<Vec<Value>>);

    impl Transport for Mock {
        fn post(&self, _: &str, _: &[(&str, &str)], body: &Value) -> Result<(u16, Value), String> {
            self.0.borrow_mut().push(body.clone());
            let reply = json!({"content": [{"type": "text", "text": "It is about cats [p. 1]."}], "usage": {"input_tokens": 40, "output_tokens": 9}});
            Ok((200, reply))
        }
    }

    fn setup() -> Setup {
        Setup {
            provider: Provider::Anthropic,
            model: "claude-x".into(),
            key: "k".into(),
        }
    }

    #[test]
    fn questions_are_answered_and_kept_in_the_documents_chat() {
        // The chat and today's usage go under a scratch %APPDATA%.
        let appdata =
            std::env::temp_dir().join(format!("mp-bridge-{}-appdata", std::process::id()));
        unsafe { std::env::set_var("APPDATA", &appdata) };
        let mock = Mock(RefCell::default());
        let hash = "00ff00ff00ff00ff";

        let ask = json!({"type": "ask", "hash": hash, "prompt": "What is it?", "pages": ["Cats."]});
        let answer = reply(&ask, &mock, setup).unwrap();
        assert_eq!(answer["type"], "answer", "{answer}");
        assert_eq!(answer["turn"]["text"], "It is about cats [p. 1].");
        assert_eq!(answer["usage"], "today 40 in · 9 out");
        assert!(
            mock.0.borrow()[0]["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("[p. 1]\nCats.")
        );

        let chat = reply(&json!({"type": "chat", "hash": hash}), &mock, setup).unwrap();
        assert_eq!(chat["turns"].as_array().unwrap().len(), 2);
        // The next question carries the chat so far.
        reply(&ask, &mock, setup).unwrap();
        assert_eq!(mock.0.borrow()[1]["messages"].as_array().unwrap().len(), 5);

        let empty = reply(&json!({"type": "new-chat", "hash": hash}), &mock, setup).unwrap();
        assert_eq!(empty, json!({"type": "chat", "turns": []}));
        assert!(mp_ai::load_chat(hash).is_empty());

        let status = reply(&json!({"type": "assistant"}), &mock, setup).unwrap();
        assert_eq!(status["model"], "Anthropic · claude-x");
        assert_eq!(status["notice"], "");
        assert_eq!(status["usage"], "today 80 in · 18 out");

        let unset = reply(&ask, &mock, Setup::default).unwrap();
        assert_eq!(unset["type"], "error");
        assert!(reply(&json!({"type": "hello"}), &mock, setup).is_none());
        let _ = std::fs::remove_dir_all(appdata);
    }

    #[test]
    fn a_long_chat_sends_its_newest_turns() {
        let turns: Vec<Turn> = (0..40)
            .map(|i| {
                if i % 2 == 0 {
                    Turn::user(&"q".repeat(50_000))
                } else {
                    Turn::assistant("a", String::new())
                }
            })
            .collect();
        let sent = chat(&turns);
        let kept = sent["turns"].as_array().unwrap();
        assert!(kept.len() < 40 && kept.len().is_multiple_of(2));
        assert_eq!(kept[0]["role"], "user");
        assert!(sent.to_string().len() <= MAX_REPLY + 100);
    }
}
