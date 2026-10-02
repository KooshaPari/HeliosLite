use forge_domain::{ChatResponse, ChatResponseContent, InteractionKind, InteractionRequest};
use std::sync::mpsc::SyncSender;

pub(super) fn response(
    output: &Option<SyncSender<String>>,
    response: &ChatResponse,
    buffer: &mut String,
) {
    let Some(output) = output else { return };
    match response {
        ChatResponse::TaskMessage { content: ChatResponseContent::Markdown { text, .. } } => {
            buffer.push_str(text);
            while let Some(index) = buffer.find('\n') {
                let line: String = buffer.drain(..=index).collect();
                let _ = output.send(line);
            }
        }
        ChatResponse::ToolCallStart { tool_call, .. } => {
            flush(output, buffer);
            let _ = output.send(format!("[remote tool] {}", tool_call.name));
        }
        ChatResponse::TaskComplete => flush(output, buffer),
        _ => {}
    }
}

pub(super) fn flush(output: &SyncSender<String>, buffer: &mut String) {
    if !buffer.is_empty() {
        let _ = output.send(std::mem::take(buffer));
    }
}

pub(super) fn interaction(output: &Option<SyncSender<String>>, request: &InteractionRequest) {
    let Some(output) = output else { return };
    let mut text = format!(
        "[remote interaction {}]\n{}",
        request.request_id, request.message
    );
    for (index, choice) in request.choices.iter().enumerate() {
        text.push_str(&format!("\n  {}. {choice}", index + 1));
    }
    let shape = if matches!(request.kind, InteractionKind::Text) {
        "your answer"
    } else {
        "choice numbers, e.g. 1 or 1,2"
    };
    text.push_str(&format!(
        "\n/respond {} {shape}\nUse /respond {} --cancel to decline.",
        request.request_id, request.request_id
    ));
    let _ = output.send(text);
}
