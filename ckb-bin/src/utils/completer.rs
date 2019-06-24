use std::borrow::Cow::{self, Owned};
use std::collections::HashSet;
use std::iter;
use std::sync::Arc;

use ansi_term::Colour::{Green, Red};
use rustyline::completion::{extract_word, Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::{CompletionType, Context, Helper};

#[cfg(unix)]
static DEFAULT_BREAK_CHARS: [u8; 18] = [
    b' ', b'\t', b'\n', b'"', b'\\', b'\'', b'`', b'@', b'$', b'>', b'<', b'=', b';', b'|', b'&',
    b'{', b'(', b'\0',
];

#[cfg(unix)]
static ESCAPE_CHAR: Option<char> = Some('\\');
// Remove \ to make file completion works on windows
#[cfg(windows)]
static DEFAULT_BREAK_CHARS: [u8; 17] = [
    b' ', b'\t', b'\n', b'"', b'\'', b'`', b'@', b'$', b'>', b'<', b'=', b';', b'|', b'&', b'{',
    b'(', b'\0',
];
#[cfg(windows)]
static ESCAPE_CHAR: Option<char> = None;

pub struct CkbCompleter<'a, 'b>
where
    'a: 'b,
{
    clap_app: Arc<clap::App<'a, 'b>>,
}

impl<'a, 'b> CkbCompleter<'a, 'b> {
    pub fn new(clap_app: clap::App<'a, 'b>) -> Self {
        CkbCompleter {
            clap_app: Arc::new(clap_app),
        }
    }

    pub fn get_completions(app: &Arc<clap::App<'a, 'b>>, args: &[String]) -> Vec<(String, String)> {
        let args_set = args.iter().collect::<HashSet<&String>>();
        let switched_completions =
            |short: Option<char>, long: Option<&str>, multiple: bool, required: bool| {
                let names = vec![
                    short.map(|s| format!("-{}", s)),
                    long.map(|s| format!("--{}", s)),
                ]
                .into_iter()
                .filter_map(|s| s)
                .map(|s| {
                    let display = if required {
                        format!("{}(*)", s)
                    } else {
                        s.clone()
                    };
                    (display, s)
                })
                .collect::<Vec<(String, String)>>();

                if !multiple && names.iter().any(|(_, s)| args_set.contains(&s)) {
                    vec![]
                } else {
                    names
                }
            };
        app.p
            .subcommands()
            .map(|app| {
                [
                    vec![(app.p.meta.name.clone(), app.p.meta.name.clone())],
                    app.p
                        .meta
                        .aliases
                        .as_ref()
                        .map(|aliases| {
                            aliases
                                .iter()
                                .map(|(alias, _)| (alias.to_string(), alias.to_string()))
                                .collect::<Vec<(String, String)>>()
                        })
                        .unwrap_or_else(|| vec![]),
                ]
                .concat()
            })
            .chain(app.p.flags().map(|a| {
                switched_completions(
                    a.s.short,
                    a.s.long,
                    a.b.is_set(clap::ArgSettings::Multiple),
                    a.b.is_set(clap::ArgSettings::Required),
                )
            }))
            .chain(app.p.opts().map(|a| {
                switched_completions(
                    a.s.short,
                    a.s.long,
                    a.b.is_set(clap::ArgSettings::Multiple),
                    a.b.is_set(clap::ArgSettings::Required),
                )
            }))
            .collect::<Vec<Vec<(String, String)>>>()
            .concat()
    }

    pub fn find_subcommand<'s, Iter: iter::Iterator<Item = &'s str>>(
        app: Arc<clap::App<'a, 'b>>,
        mut prefix_names: iter::Peekable<Iter>,
    ) -> Option<Arc<clap::App<'a, 'b>>> {
        if let Some(name) = prefix_names.next() {
            for inner_app in &(app.p.subcommands) {
                if inner_app.p.meta.name == name
                    || inner_app
                        .p
                        .meta
                        .aliases
                        .as_ref()
                        .map(|aliases| aliases.iter().any(|&(alias, _)| alias == name))
                        .unwrap_or(false)
                {
                    return if prefix_names.peek().is_none() {
                        Some(Arc::new(inner_app.to_owned()))
                    } else {
                        Self::find_subcommand(Arc::new(inner_app.to_owned()), prefix_names)
                    };
                }
            }
        }
        if prefix_names.peek().is_none() || app.p.subcommands.is_empty() {
            Some(app)
        } else {
            None
        }
    }
}

impl<'a, 'b> Completer for CkbCompleter<'a, 'b> {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _context: &Context,
    ) -> Result<(usize, Vec<Pair>), ReadlineError> {
        let (start, word) = extract_word(line, pos, ESCAPE_CHAR, &DEFAULT_BREAK_CHARS);
        let args = shell_words::split(&line[..pos]).unwrap();
        let word_lower = word.to_lowercase();
        let tmp_pair = Self::find_subcommand(
            Arc::<clap::App<'a, 'b>>::clone(&self.clap_app),
            args.iter().map(String::as_str).peekable(),
        )
        .map(|current_app| Self::get_completions(&current_app, &args))
        .unwrap_or_default();

        if word_lower.is_empty() {
            let pairs = tmp_pair
                .clone()
                .into_iter()
                .map(|(display, replacement)| Pair {
                    display,
                    replacement,
                })
                .collect::<Vec<_>>();
            Ok((start, pairs))
        } else {
            let pairs = tmp_pair
                .clone()
                .into_iter()
                .filter(|(_, replacement)| string_include(&replacement.to_lowercase(), &word_lower))
                .map(|(display, replacement)| Pair {
                    display,
                    replacement,
                })
                .collect::<Vec<_>>();

            if pairs
                .iter()
                .any(|ref mut x| x.replacement.to_lowercase().contains(&word_lower))
            {
                let pairs = tmp_pair
                    .clone()
                    .into_iter()
                    .filter(|(_, replacement)| replacement.to_lowercase().contains(&word_lower))
                    .map(|(display, replacement)| Pair {
                        display,
                        replacement,
                    })
                    .collect::<Vec<_>>();
                Ok((start, pairs))
            } else {
                let pairs = tmp_pair
                    .into_iter()
                    .filter(|(_, replacement)| {
                        string_include(&replacement.to_lowercase(), &word_lower)
                    })
                    .map(|(display, replacement)| Pair {
                        display,
                        replacement,
                    })
                    .collect::<Vec<_>>();
                Ok((start, pairs))
            }
        }
    }
}

impl<'a, 'b> Helper for CkbCompleter<'a, 'b> {}

impl<'a, 'b> Highlighter for CkbCompleter<'a, 'b> {
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Owned("\x1b[1m".to_owned() + hint + "\x1b[m")
    }

    fn highlight_candidate<'c>(
        &self,
        candidate: &'c str,
        _completion: CompletionType,
    ) -> Cow<'c, str> {
        let candidate_with_color = candidate
            .split('\n')
            .map(|param| {
                if param.contains('*') {
                    Red.paint(param).to_string()
                } else if !param.starts_with("--") {
                    Green.paint(param).to_string()
                } else {
                    param.to_string()
                }
            })
            .collect::<Vec<String>>()
            .join("\n");
        Owned(candidate_with_color)
    }
}

impl<'a, 'b> Hinter for CkbCompleter<'a, 'b> {
    fn hint(&self, _line: &str, _pos: usize, _context: &Context) -> Option<String> {
        None
    }
}

pub fn string_include(replacement: &str, word: &str) -> bool {
    let len_pat = word.len();
    let replacement_chars: Vec<char> = replacement.chars().collect();
    let word_chars: Vec<char> = word.chars().collect();

    let mut sum = 0;

    for item in replacement_chars {
        if item == word_chars[sum] {
            sum += 1;
            if sum == len_pat {
                return true;
            }
        }
    }
    false
}
