// Unix hosts use CSI cursor queries; Windows uses its native console API.
#![cfg(unix)]

use super::common::*;

const PASTE_READY: &[u8] = b"\x1b[?2004h";
const CURSOR_QUERY: &[u8] = b"\x1b[6n";
const STARTUP_PROMPT: &str = "STARTUP_CANARY";
const RESPONSE: &str = "STARTUP_ACK";

async fn paste_at_readiness_reaches_agent(respond_to_queries: bool) {
    let content = ContentController::start().await.expect("start content");
    content.set_response(RESPONSE);
    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--no-alt-screen"],
    )
    .expect("spawn inline pager");
    harness.set_respond_to_queries(respond_to_queries);
    harness
        .wait_until("paste readiness", WELCOME_TIMEOUT, |h| {
            h.raw_output()
                .windows(PASTE_READY.len())
                .any(|w| w == PASTE_READY)
        })
        .expect("pager advertises paste readiness");

    // Hosts inject the initial prompt on this escape, before the first frame.
    harness
        .inject_keys(format!("\x1b[200~{STARTUP_PROMPT}\x1b[201~\r").as_bytes())
        .expect("paste initial prompt immediately");
    harness
        .wait_for_text(RESPONSE, Duration::from_secs(30))
        .expect("startup paste reaches the model and renders its reply");
    assert!(
        content
            .request_bodies()
            .iter()
            .any(|body| body.to_string().contains(STARTUP_PROMPT)),
        "the model must receive the injected prompt"
    );

    let raw = harness.raw_output();
    let ready = raw
        .windows(PASTE_READY.len())
        .position(|w| w == PASTE_READY)
        .unwrap();
    let queries: Vec<_> = raw
        .windows(CURSOR_QUERY.len())
        .enumerate()
        .filter_map(|(i, w)| (w == CURSOR_QUERY).then_some(i))
        .collect();
    assert!(
        !queries.is_empty(),
        "inline startup must exercise cursor probing"
    );
    if respond_to_queries {
        assert!(
            queries.len() >= 2,
            "responsive preflight must proceed to the inline cursor lookup"
        );
    }
    assert!(
        queries.iter().all(|i| *i < ready),
        "cursor probes after paste readiness can consume the injected prompt"
    );
    harness.quit().expect("clean quit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn startup_paste_survives_silent_terminal() {
    paste_at_readiness_reaches_agent(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn startup_paste_survives_cursor_replies() {
    paste_at_readiness_reaches_agent(true).await;
}
