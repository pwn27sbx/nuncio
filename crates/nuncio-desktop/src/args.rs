//! The command line, without GTK.
//!
//! The app's own options are taken here; `GApplication`'s own (`--gapplication-service`, which
//! the app's D-Bus `.service` file runs for activation in the background, and the other
//! `--gapplication-*` options) and the links go on to `GApplication`, so it handles them. Any
//! other `-`option is a usage error.

use std::path::PathBuf;

/// The app id, which `GApplication` also owns as a D-Bus name. It must differ from the daemon's
/// bus name (`io.github.pwn27sbx.nuncio`, `internal/ipc/connect.go` `DaemonName`): one name has
/// one owner, so sharing it would make the app and `nunciod` fight over it.
pub const APP_ID: &str = "io.github.pwn27sbx.nuncio.Desktop";

pub const USAGE: &str = "usage: nuncio-desktop [--theme-dir DIR] [--probe] [LINK...]\n\n  \
     --theme-dir DIR  directory holding colors.toml (default: Omarchy's)\n  \
     --probe          run the S5 theme-switch check and exit\n  \
     LINK             a whatsapp://send?phone=... or https://wa.me/... link: opens\n                   \
     that chat with the link's text in the composer (never sent)\n\n\
     GApplication's own --gapplication-* options (e.g. --gapplication-service) are passed on.";

/// What the command line asks for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub theme_dir: Option<PathBuf>,
    pub probe: bool,
    /// What `GApplication` gets after `argv[0]`: its own options, then the links, in order.
    pub passed_on: Vec<String>,
}

/// A command line that is not a run.
#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Run(Args),
    Help,
    Unexpected(String),
}

/// Parse the arguments after `argv[0]`.
pub fn parse(argv: impl IntoIterator<Item = String>) -> Parsed {
    let mut args = Args::default();
    let mut argv = argv.into_iter();
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--probe" => args.probe = true,
            "--theme-dir" => args.theme_dir = argv.next().map(PathBuf::from),
            "-h" | "--help" => return Parsed::Help,
            // Its only option with a separate value.
            "--gapplication-app-id" => {
                args.passed_on.push(arg);
                args.passed_on.extend(argv.next());
            }
            other if other.starts_with("--gapplication-") => args.passed_on.push(arg),
            other if !other.starts_with('-') => args.passed_on.push(arg),
            _ => return Parsed::Unexpected(arg),
        }
    }
    Parsed::Run(args)
}

#[cfg(test)]
mod tests {
    use super::{APP_ID, Args, Parsed, parse};
    use std::path::PathBuf;

    fn parsed(argv: &[&str]) -> Parsed {
        parse(argv.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn the_app_id_is_not_the_daemons_bus_name() {
        // `DaemonName` in internal/ipc/connect.go: an existing contract that does not move.
        const DAEMON_NAME: &str = "io.github.pwn27sbx.nuncio";
        assert_ne!(APP_ID, DAEMON_NAME);
        assert_eq!(APP_ID, "io.github.pwn27sbx.nuncio.Desktop");
    }

    #[test]
    fn gapplication_options_go_on_to_gapplication() {
        assert_eq!(
            parsed(&["--gapplication-service"]),
            Parsed::Run(Args {
                passed_on: vec!["--gapplication-service".to_owned()],
                ..Args::default()
            })
        );
        assert_eq!(
            parsed(&["--gapplication-app-id", "x.y", "--gapplication-replace"]),
            Parsed::Run(Args {
                passed_on: vec![
                    "--gapplication-app-id".to_owned(),
                    "x.y".to_owned(),
                    "--gapplication-replace".to_owned()
                ],
                ..Args::default()
            })
        );
    }

    #[test]
    fn the_apps_own_options_and_links_still_work() {
        let link = "whatsapp://send?phone=15551230001&text=hola";
        assert_eq!(
            parsed(&["--theme-dir", "/tmp/t", "--probe", link]),
            Parsed::Run(Args {
                theme_dir: Some(PathBuf::from("/tmp/t")),
                probe: true,
                passed_on: vec![link.to_owned()],
            })
        );
        assert_eq!(parsed(&["--help"]), Parsed::Help);
        assert_eq!(parsed(&["-h"]), Parsed::Help);
        assert_eq!(parsed(&[]), Parsed::Run(Args::default()));
    }

    #[test]
    fn unknown_options_are_still_rejected() {
        assert_eq!(parsed(&["--foo"]), Parsed::Unexpected("--foo".to_owned()));
        assert_eq!(parsed(&["-x"]), Parsed::Unexpected("-x".to_owned()));
        assert_eq!(
            parsed(&["--gapplication"]),
            Parsed::Unexpected("--gapplication".to_owned())
        );
    }
}
