pub const DEFAULT_SPEC: &str = "dev";
pub const AVAILABLE_SPECS: &[&str] = &["dev", "testnet"];
pub const DEFAULT_RPC_PORT: &str = "8114";
pub const DEFAULT_P2P_PORT: &str = "8115";

const START_MARKER: &str = " # {{";
const END_MAKER: &str = "# }}";
const WILDCARD_BRANCH: &str = "# _ => ";

use std::io;

pub struct Template<T>(T);

pub struct TemplateContext<'a> {
    pub spec: &'a str,
    pub rpc_port: &'a str,
    pub p2p_port: &'a str,
    pub log_to_file: bool,
    pub log_to_stdout: bool,
}

impl<'a> Default for TemplateContext<'a> {
    fn default() -> Self {
        TemplateContext {
            spec: DEFAULT_SPEC,
            rpc_port: DEFAULT_RPC_PORT,
            p2p_port: DEFAULT_P2P_PORT,
            log_to_file: true,
            log_to_stdout: true,
        }
    }
}

impl<T> Template<T> {
    pub fn new(content: T) -> Self {
        Template(content)
    }
}

fn writeln<W: io::Write>(w: &mut W, s: &str, context: &TemplateContext) -> io::Result<()> {
    writeln!(
        w,
        "{}",
        s.replace("\\n", "\n")
            .replace("{rpc_port}", context.rpc_port)
            .replace("{p2p_port}", context.p2p_port)
            .replace("{log_to_file}", &format!("{}", context.log_to_file))
            .replace("{log_to_stdout}", &format!("{}", context.log_to_stdout))
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
        let spec_branch = format!("# {} => ", context.spec).to_string();

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
