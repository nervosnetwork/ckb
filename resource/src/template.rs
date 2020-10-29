/// TODO(doc): @doitian
pub const DEFAULT_SPEC: &str = "mainnet";
/// TODO(doc): @doitian
pub const AVAILABLE_SPECS: &[&str] = &["mainnet", "testnet", "staging", "dev"];
/// TODO(doc): @doitian
pub const DEFAULT_RPC_PORT: &str = "8114";
/// TODO(doc): @doitian
pub const DEFAULT_P2P_PORT: &str = "8115";

const START_MARKER: &str = " # {{";
const END_MAKER: &str = "# }}";
const WILDCARD_BRANCH: &str = "# _ => ";

use std::collections::HashMap;
use std::io;

pub struct Template<T>(T);

/// TODO(doc): @doitian
pub struct TemplateContext<'a> {
    spec: &'a str,
    kvs: HashMap<&'a str, &'a str>,
}

impl<'a> TemplateContext<'a> {
    /// TODO(doc): @doitian
    pub fn new<I>(spec: &'a str, kvs: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        Self {
            spec,
            kvs: kvs.into_iter().collect(),
        }
    }

    /// TODO(doc): @doitian
    pub fn insert(&mut self, key: &'a str, value: &'a str) {
        self.kvs.insert(key, value);
    }
}

impl<T> Template<T> {
    pub fn new(content: T) -> Self {
        Template(content)
    }
}

fn writeln<W: io::Write>(w: &mut W, s: &str, context: &TemplateContext) -> io::Result<()> {
    #[cfg(docker)]
    let s = s.replace("127.0.0.1:{rpc_port}", "0.0.0.0:{rpc_port}");
    writeln!(
        w,
        "{}",
        context
            .kvs
            .iter()
            .fold(s.replace("\\n", "\n"), |s, (key, value)| s
                .replace(format!("{{{}}}", key).as_str(), value))
    )
}

#[derive(Debug)]
pub enum TemplateState<'a> {
    SearchStartMarker,
    MatchBranch(&'a str),
    SearchEndMarker,
}

impl<T> Template<T>
where
    T: AsRef<str>,
{
    pub fn write_to<'c, W: io::Write>(
        &self,
        w: &mut W,
        context: &TemplateContext<'c>,
    ) -> io::Result<()> {
        let spec_branch = format!("# {} => ", context.spec);

        let mut state = TemplateState::SearchStartMarker;
        for line in self.0.as_ref().lines() {
            // dbg!((line, &state));
            match state {
                TemplateState::SearchStartMarker => {
                    if line.ends_with(START_MARKER) {
                        state = TemplateState::MatchBranch(line);
                    } else {
                        writeln!(w, "{}", line)?;
                    }
                }
                TemplateState::MatchBranch(start_line) => {
                    if line == END_MAKER {
                        writeln!(
                            w,
                            "{}",
                            &start_line[..(start_line.len() - START_MARKER.len())],
                        )?;
                        state = TemplateState::SearchStartMarker;
                    } else if line.starts_with(&spec_branch) {
                        writeln(w, &line[spec_branch.len()..], context)?;
                        state = TemplateState::SearchEndMarker;
                    } else if line.starts_with(WILDCARD_BRANCH) {
                        writeln(w, &line[WILDCARD_BRANCH.len()..], context)?;
                        state = TemplateState::SearchEndMarker;
                    }
                }
                TemplateState::SearchEndMarker => {
                    if line == END_MAKER {
                        state = TemplateState::SearchStartMarker;
                    }
                }
            }
        }

        if let TemplateState::MatchBranch(start_line) = state {
            writeln!(
                w,
                "{}",
                &start_line[..(start_line.len() - START_MARKER.len())],
            )?;
        }

        Ok(())
    }
}
