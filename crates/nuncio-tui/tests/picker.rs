//! The `ratatui-image` half of the S9 check: does this library detect the kitty graphics
//! protocol in a Ghostty terminal?
//!
//! Findings, from reading `ratatui-image` 11.1.0's `picker` module and confirmed here:
//!
//! * the picker does **not** take the protocol from `TERM_PROGRAM`. `Picker::from_query_stdio`
//!   writes a kitty graphics probe to the terminal and believes the terminal's *answer*;
//!   `TERM_PROGRAM`/`TERM` are only consulted for tmux and iTerm2 hints (and
//!   `Picker::from_fontsize` is deprecated in 11.x).
//! * so the behaviour to pin is "Ghostty's answer to that probe ⇒ `ProtocolType::Kitty`",
//!   which is what `ghosttys_answer_to_the_query_is_kitty_support` does through the public
//!   `cap_parser`.
//!
//! The end-to-end half (an actual run rendering a PNG in Ghostty) is reported in the
//! task handoff, not here, because it needs a real tty.

use std::io::IsTerminal;

use ratatui_image::picker::Picker;
use ratatui_image::picker::ProtocolType;
use ratatui_image::picker::cap_parser::{Parser, QueryStdioOptions, Response};

/// What Ghostty sends back for the kitty graphics probe.
const KITTY_ANSWER: &str = "\x1b_Gi=31;OK\x1b\\";

#[test]
fn the_picker_probes_the_terminal_for_kitty_graphics() {
    let query = Parser::query(false, QueryStdioOptions::default());

    assert!(
        query.contains("\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\"),
        "the kitty graphics probe is part of the query"
    );
    assert!(
        query.contains("\x1b[5n"),
        "the device status request that ends the read loop"
    );
}

#[test]
fn ghosttys_answer_to_the_query_is_kitty_support() {
    let mut parser = Parser::new();
    let mut responses = Vec::new();
    for character in KITTY_ANSWER.chars() {
        responses.extend(parser.push(character));
    }

    assert_eq!(
        responses,
        vec![Response::Kitty],
        "Ghostty answers the kitty graphics probe, so the picker selects the kitty protocol"
    );
}

#[test]
fn a_terminal_that_does_not_answer_never_yields_kitty() {
    if std::io::stdin().is_terminal() {
        // A real terminal (Ghostty here) does answer the query, and then the protocol *is*
        // kitty. This test only has something to say when there is nothing to answer with.
        return;
    }

    match Picker::from_query_stdio() {
        Ok(picker) => assert_ne!(
            picker.protocol_type(),
            ProtocolType::Kitty,
            "without an answer from the terminal there is nothing to detect kitty from"
        ),
        Err(_) => {
            // Also fine: no tty to query at all. Either way, TERM_PROGRAM=ghostty alone
            // is not what selects the protocol.
        }
    }
}

#[test]
fn the_fallback_picker_is_halfblocks() {
    // What `nuncio-tui` falls back to when the query fails: still renders, no kitty
    // escape sequences.
    assert_eq!(
        Picker::halfblocks().protocol_type(),
        ProtocolType::Halfblocks
    );
}
