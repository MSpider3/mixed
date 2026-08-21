use std::path::PathBuf;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAME: &str = env!("CARGO_PKG_NAME");
pub const DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");

#[derive(Debug, PartialEq, Eq)]
pub enum CliAction {
    Help,
    Version,
    Run(CliOptions),
}

#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct CliOptions {
    pub music_dir: Option<PathBuf>,
    pub play_file: Option<PathBuf>,
}

impl CliOptions {
    pub fn parse<I, T>(args: I) -> CliAction
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        let args: Vec<String> = args.into_iter().map(Into::into).collect();
        // Skip program name if present
        let args_slice = if args.is_empty() {
            &args[..]
        } else {
            &args[1..]
        };

        let mut opts = CliOptions::default();
        let mut iter = args_slice.iter().peekable();

        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "-h" | "--help" => return CliAction::Help,
                "-v" | "-V" | "--version" => return CliAction::Version,
                "-d" | "--dir" => {
                    if let Some(next) = iter.next() {
                        opts.music_dir = Some(PathBuf::from(next));
                    }
                }
                "-p" | "--play" => {
                    if let Some(next) = iter.next() {
                        opts.play_file = Some(PathBuf::from(next));
                    }
                }
                _ if arg.starts_with("--dir=") => {
                    let val = &arg["--dir=".len()..];
                    opts.music_dir = Some(PathBuf::from(val));
                }
                _ if arg.starts_with("--play=") => {
                    let val = &arg["--play=".len()..];
                    opts.play_file = Some(PathBuf::from(val));
                }
                _ if !arg.starts_with('-') => {
                    let path = PathBuf::from(arg);
                    if path.is_dir() {
                        opts.music_dir = Some(path);
                    } else {
                        // If file or unknown path, treat as play target
                        opts.play_file = Some(path);
                    }
                }
                _ => {
                    // Unknown flag: treat as request for help or skip
                    if arg.starts_with('-') {
                        eprintln!("Unknown option: {}", arg);
                        return CliAction::Help;
                    }
                }
            }
        }

        CliAction::Run(opts)
    }
}

pub fn print_help() {
    println!(
        "{name} {version} - {description}\n\
        \n\
        USAGE:\n    \
            {name} [OPTIONS] [PATH]\n\
        \n\
        ARGS:\n    \
            <PATH>               Path to a music directory or audio file to play directly\n\
        \n\
        OPTIONS:\n    \
            -d, --dir <DIR>      Open {name} with the specified music library directory\n    \
            -p, --play <FILE>    Directly play the specified audio file on startup\n    \
            -h, --help           Print help information\n    \
            -v, -V, --version    Print version information\n\
        \n\
        KEYBOARD SHORTCUTS:\n    \
            F1-F5                Switch views (Help, Queue, Library, Search, Full Lyrics)\n    \
            Space                Play / Pause\n    \
            Left / Right (h/l)   Seek backward / forward 5s (Shift for 30s)\n    \
            Up / Down (j/k)      Navigate lists / tracks\n    \
            Enter                Play selected track / expand folder\n    \
            Tab                  Cycle active panels\n    \
            v                    Toggle visualizer\n    \
            s                    Toggle shuffle mode\n    \
            r                    Cycle repeat mode (Off, All, One)\n    \
            + / - (m/M)          Adjust volume / mute\n    \
            q, Ctrl+C            Quit\n",
        name = NAME,
        version = VERSION,
        description = DESCRIPTION
    );
}

pub fn print_version() {
    println!("{} {}", NAME, VERSION);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_help() {
        assert_eq!(CliOptions::parse(vec!["mixed", "--help"]), CliAction::Help);
        assert_eq!(CliOptions::parse(vec!["mixed", "-h"]), CliAction::Help);
    }

    #[test]
    fn test_cli_version() {
        assert_eq!(
            CliOptions::parse(vec!["mixed", "--version"]),
            CliAction::Version
        );
        assert_eq!(CliOptions::parse(vec!["mixed", "-v"]), CliAction::Version);
        assert_eq!(CliOptions::parse(vec!["mixed", "-V"]), CliAction::Version);
    }

    #[test]
    fn test_cli_dir_flag() {
        assert_eq!(
            CliOptions::parse(vec!["mixed", "--dir", "/path/to/music"]),
            CliAction::Run(CliOptions {
                music_dir: Some(PathBuf::from("/path/to/music")),
                play_file: None,
            })
        );
    }

    #[test]
    fn test_cli_play_flag() {
        assert_eq!(
            CliOptions::parse(vec!["mixed", "--play", "/path/to/song.mp3"]),
            CliAction::Run(CliOptions {
                music_dir: None,
                play_file: Some(PathBuf::from("/path/to/song.mp3")),
            })
        );
    }
}
