pub const DEFAULT_SPEC: &str = "mainnet";
pub const AVAILABLE_SPECS: &[&str] = &["mainnet", "testnet", "staging", "dev"];
pub const DEFAULT_RPC_PORT: &str = "8114";
pub const DEFAULT_P2P_PORT: &str = "8115";

const START_MARKER: &str = " # {{";
const END_MAKER: &str = "# }}";
const WILDCARD_BRANCH: &str = "# _ => ";

use std::io;

pub struct Template<T>(T);

pub struct TemplateContext<'a> {
    pub spec: &'a str,
    pub spec_source: &'a str,
    pub rpc_port: &'a str,
    pub p2p_port: &'a str,
    pub log_to_file: bool,
    pub log_to_stdout: bool,
    pub block_assembler: &'a str,
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
        s.replace("\\n", "\n")
            .replace("{rpc_port}", context.rpc_port)
            .replace("{p2p_port}", context.p2p_port)
            .replace("{log_to_file}", &format!("{}", context.log_to_file))
            .replace("{log_to_stdout}", &format!("{}", context.log_to_stdout))
            .replace("{block_assembler}", context.block_assembler)
            .replace("{spec_source}", context.spec_source)
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
