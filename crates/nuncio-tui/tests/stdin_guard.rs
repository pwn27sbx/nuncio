//! The guard behind the startup image query: ratatui-image 11.1 leaves its reader thread blocked
//! on stdin when the terminal does not answer in time (issue #202). Holding Rust's stdin lock for
//! the rest of the run keeps that thread from ever reading again, so it cannot steal a key.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use nuncio_tui::images::hold_stdin;

#[test]
fn holding_stdin_keeps_every_other_reader_out() {
    let held = hold_stdin();
    held.recv_timeout(Duration::from_secs(5))
        .expect("the lock is taken when nobody else holds it");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _lock = std::io::stdin().lock();
        let _ = tx.send(());
    });
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "another reader waits for ever"
    );
}
