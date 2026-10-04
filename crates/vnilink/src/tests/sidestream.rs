use crate::error::Error;
use crate::sidestream::await_opened;

#[tokio::test]
async fn an_open_stream_reports_its_task() {
    let task = tokio::spawn(async {});
    let (opened_tx, opened_rx) = tokio::sync::oneshot::channel();
    let _ = opened_tx.send(Ok(()));
    let returned = await_opened(task, opened_rx, "the clipboard stream did not open")
        .await
        .unwrap();
    returned.await.unwrap();
}

#[tokio::test]
async fn a_refused_stream_returns_the_refusal() {
    let task = tokio::spawn(async {});
    let (opened_tx, opened_rx) = tokio::sync::oneshot::channel();
    let _ = opened_tx.send(Err(Error::ClipboardDenied));
    assert!(matches!(
        await_opened(task, opened_rx, "the clipboard stream did not open").await,
        Err(Error::ClipboardDenied)
    ));
}

#[tokio::test]
async fn a_silent_stream_reports_that_it_did_not_open() {
    let task = tokio::spawn(async {});
    let (opened_tx, opened_rx) = tokio::sync::oneshot::channel::<Result<(), Error>>();
    drop(opened_tx);
    assert!(matches!(
        await_opened(task, opened_rx, "the transfer stream did not open").await,
        Err(Error::Network(_))
    ));
}
