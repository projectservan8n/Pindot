//! Keeping a conversation inside the model's context: an estimate of its
//! size, and trimming in a fixed order (old pictures, then long old tool
//! results, then the oldest whole turns).

use super::openai::Msg;

/// About what one screenshot costs a model that can see.
pub const IMAGE_TOKENS: usize = 1100;
/// Old tool results are cut to this many characters when trimming.
pub const SHORT_RESULT: usize = 300;

/// A rough token count: four characters a token, plus pictures.
pub fn estimate(messages: &[Msg]) -> usize {
    messages
        .iter()
        .map(|m| {
            4 + match m {
                Msg::System(t) | Msg::Tool { text: t, .. } => t.len() / 4,
                Msg::User { text, images } => text.len() / 4 + images.len() * IMAGE_TOKENS,
                Msg::Assistant { text, calls } => {
                    (text.len() + calls.iter().map(|c| c.name.len() + c.arguments.len()).sum::<usize>()) / 4
                }
                Msg::ToolImages(i) => i.len() * IMAGE_TOKENS,
            }
        })
        .sum()
}

/// Trims `messages` (index 0, the system prompt, always stays) until it
/// fits in `budget` tokens, or only the last turn is left. Returns whether
/// anything changed.
pub fn fit(messages: &mut Vec<Msg>, budget: usize) -> bool {
    if estimate(messages) <= budget {
        return false;
    }
    // 1. Every tool picture but the latest.
    let last_pic = messages.iter().rposition(|m| matches!(m, Msg::ToolImages(_)));
    let mut i = 0;
    messages.retain(|m| {
        let keep = !matches!(m, Msg::ToolImages(_)) || Some(i) == last_pic;
        i += 1;
        keep
    });
    if estimate(messages) <= budget {
        return true;
    }
    // 2. Long tool results, all but the last two.
    let tools: Vec<usize> = messages.iter().enumerate().filter(|(_, m)| matches!(m, Msg::Tool { .. })).map(|(i, _)| i).collect();
    for &i in tools.iter().rev().skip(2) {
        if let Msg::Tool { text, .. } = &mut messages[i]
            && text.len() > SHORT_RESULT
        {
            let cut: String = text.chars().take(SHORT_RESULT).collect();
            *text = format!("{cut}... (shortened)");
        }
    }
    // 3. The oldest whole turns (a turn starts at a user message), never the last.
    while estimate(messages) > budget {
        let users: Vec<usize> = messages.iter().enumerate().filter(|(_, m)| matches!(m, Msg::User { .. })).map(|(i, _)| i).collect();
        if users.len() < 2 {
            break;
        }
        messages.drain(users[0]..users[1]);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::native::openai::{Image, ToolCall};

    fn img() -> Image {
        Image { mime: "image/png".into(), base64: "A".repeat(100) }
    }

    fn turn(n: usize) -> Vec<Msg> {
        vec![
            Msg::User { text: format!("request {n}"), images: vec![] },
            Msg::Assistant {
                text: String::new(),
                calls: vec![ToolCall { id: format!("c{n}"), name: "screenshot".into(), arguments: "{}".into() }],
            },
            Msg::Tool { call_id: format!("c{n}"), text: "x".repeat(2000) },
            Msg::ToolImages(vec![img()]),
            Msg::Assistant { text: format!("done {n}"), calls: vec![] },
        ]
    }

    #[test]
    fn nothing_changes_when_it_fits() {
        let mut m = vec![Msg::System("s".into())];
        m.extend(turn(1));
        let before = m.clone();
        assert!(!fit(&mut m, 100_000));
        assert_eq!(m, before);
    }

    #[test]
    fn old_pictures_go_first_then_long_results_then_old_turns() {
        let mut m = vec![Msg::System("s".into())];
        for n in 0..4 {
            m.extend(turn(n));
        }
        let pics = |m: &[Msg]| m.iter().filter(|x| matches!(x, Msg::ToolImages(_))).count();
        assert_eq!(pics(&m), 4);
        // Just over: only the old pictures go.
        let budget = estimate(&m) - 2 * IMAGE_TOKENS;
        assert!(fit(&mut m, budget));
        assert_eq!(pics(&m), 1, "the latest picture stays");
        assert_eq!(m.iter().filter(|x| matches!(x, Msg::User { .. })).count(), 4, "no turn dropped yet");
        // Much tighter: old turns go; the system prompt and the last turn stay.
        assert!(fit(&mut m, 900));
        assert!(matches!(m[0], Msg::System(_)));
        assert!(matches!(&m[1], Msg::User { text, .. } if text == "request 3"), "{m:?}");
    }

    #[test]
    fn long_old_results_are_shortened_before_turns_go() {
        let mut m = vec![Msg::System("s".into())];
        for n in 0..4 {
            m.extend(turn(n).into_iter().filter(|x| !matches!(x, Msg::ToolImages(_))));
        }
        let budget = estimate(&m) - 600;
        assert!(fit(&mut m, budget));
        assert_eq!(m.iter().filter(|x| matches!(x, Msg::User { .. })).count(), 4, "no turn dropped");
        let short = m.iter().filter(|x| matches!(x, Msg::Tool { text, .. } if text.ends_with("(shortened)"))).count();
        assert_eq!(short, 2, "all but the last two results");
    }
}
