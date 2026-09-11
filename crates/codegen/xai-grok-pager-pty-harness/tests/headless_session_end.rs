//! Exercise headless memory flush and SessionEnd against the local mock server.
#![cfg(unix)]

use std::time::Duration;

use xai_grok_pager_pty_harness::{pager_binary, ContentController};
use xai_grok_test_support::{
    assert_headless_success, run_headless_in_sandbox_borrowed_with_env, sse, InferenceEndpoint,
    InferenceRequestMatcher, ScriptedResponse,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn memory_flush_finishes_before_session_end() {
    let content = ContentController::start().await.expect("start content");
    content.set_response("ACK");
    let sandbox = content.sandbox();
    std::fs::write(
        sandbox.grok_home().join("config.toml"),
        r#"
[memory]
enabled = true
mode = "legacy"
[memory.dream]
enabled = false
[memory.session]
save_on_end = false
[memory.initial_injection]
enabled = false
"#,
    )
    .expect("configure memory flush");
    let hook_dir = sandbox.grok_home().join("hooks");
    std::fs::create_dir_all(&hook_dir).expect("create hook directory");
    std::fs::write(
        hook_dir.join("session-end.json"),
        serde_json::json!({"hooks": {"SessionEnd": [{"hooks": [{
            "type": "command",
            "command": "printf done > \"$SESSION_END_MARKER\""
        }]}]}})
        .to_string(),
    )
    .expect("install SessionEnd hook");
    let marker = sandbox.grok_home().join("session-ended");
    let server = content.server();
    let mut responses = server.expect_response_blocked(
        "memory flush responses",
        InferenceRequestMatcher::auxiliary(InferenceEndpoint::Responses),
        ScriptedResponse::sse(sse::responses_api_script_exact("NO_REPLY", "test-model")),
    );
    let mut chat = server.expect_response_blocked(
        "memory flush chat",
        InferenceRequestMatcher::auxiliary(InferenceEndpoint::ChatCompletions),
        ScriptedResponse::sse(sse::chat_completion_script_exact("NO_REPLY", "test-model")),
    );
    let mut cmd = tokio::process::Command::new(pager_binary().expect("resolve pager binary"));
    cmd.current_dir(sandbox.workspace()).args([
        "-p",
        "memory-order-canary",
        "--model",
        "test-model",
        "--memory-flush",
        "--output-format",
        "streaming-json",
        "--yolo",
    ]);
    let overrides = [
        ("GROK_TITLE_REFRESH", "0"),
        ("GROK_GOAL_SUMMARY", "0"),
        ("SESSION_END_MARKER", marker.to_str().unwrap()),
    ];
    let run = run_headless_in_sandbox_borrowed_with_env(cmd, sandbox, &overrides);
    tokio::pin!(run);
    tokio::select! {
        result = &mut run => panic!("headless exited before memory flush: {}", result.stderr),
        _ = responses.wait_blocked() => {},
        _ = chat.wait_blocked() => {},
        _ = tokio::time::sleep(Duration::from_secs(30)) => panic!("memory flush did not start"),
    }
    assert!(
        server
            .requests()
            .iter()
            .any(|request| request.headers.iter().any(|(name, value)| {
                name == "x-grok-req-id" && value.starts_with("xai-flush-")
            })),
        "the blocked auxiliary request must be the memory flush"
    );
    assert!(!marker.exists(), "SessionEnd must wait for memory flush");
    responses.release();
    chat.release();

    let result = run.await;
    assert_headless_success(
        &result,
        "memory flush before session shutdown",
        Some(server),
    );
    let events: Vec<serde_json::Value> = result
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("streaming JSON event"))
        .collect();
    assert!(
        events
            .iter()
            .any(|event| event["type"] == "memory_flush_completed"
                && event["result"] == "nothing to store"),
        "flush must complete: {}",
        result.stdout
    );
    assert_eq!(
        std::fs::read_to_string(&marker).expect("SessionEnd hook must run"),
        "done"
    );
}
