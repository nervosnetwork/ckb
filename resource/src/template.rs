/// Default chain spec.
pub const DEFAULT_SPEC: &str = "mainnet";
/// The list of bundled chain specs.
pub const AVAILABLE_SPECS: &[&str] = &["mainnet", "testnet", "staging", "dev"];
/// The default RPC listen port *8114*.
pub const DEFAULT_RPC_PORT: &str = "8114";
/// The default P2P listen port *8115*.
pub const DEFAULT_P2P_PORT: &str = "8115";

const START_MARKER: &str = " # {{";
const END_MAKER: &str = "# }}";
const WILDCARD_BRANCH: &str = "# _ => ";

use std::collections::HashMap;
use std::io;

/// A simple template which supports spec branches and variables.
///
/// The template is designed so that without expanding the template, it is still a valid TOML file.
///
/// ### Spec Branches
///
/// A spec branches block replaces a line with a branch matching the given spec name.
///
/// The block starts with the line ending with ` # {{` (the leading space is required) and ends
/// with a line `# }}`.
///
/// Between the start and end markers, every line is a branch starting with `# SPEC => CONTENT`, where
/// `SPEC` is the branch spec name, and `CONTENT` is the text to be replaced for the spec.
/// A special spec name `_` acts as a wildcard which matches any spec name.
///
/// The spec name is required to render the template, see [`Template::new`]. The block including
/// the **whole** starting line which ends with ` # {{` will be replaced by the branch `CONTENT`
/// which `SPEC` is `_` or equals to the given spec name.
///
/// In the `CONTENT`, variables are expanded and all the escape sequences `\n` are replaced by new
/// lines.
///
/// ```
/// use ckb_resource::{Template, TemplateContext};
///
/// let template = Template::new(
///     r#"filter = "debug" # {{
/// ## mainnet => filter = "error"
/// ## _ => filter = "info"
/// ## }}"#
///         .to_string(),
/// );
/// let mainnet_result = template.render(&TemplateContext::new("mainnet", Vec::new()));
/// assert_eq!("filter = \"error\"\n", mainnet_result.unwrap());
/// let testnet_result = template.render(&TemplateContext::new("testnet", Vec::new()));
/// assert_eq!("filter = \"info\"\n", testnet_result.unwrap());
/// ```
///
/// ### Template Variables
///
/// Template variables are defined as key value dictionary in [`TemplateContext`] via
/// [`TemplateContext::new`] or [`TemplateContext::insert`].
///
/// Template uses variables by surrounding the variable names with curly brackets.
///
/// The variables expansions **only** happen inside the spec branches in the spec `CONTENT`.
/// It is a trick to use a wildcard branch as in the following example.
///
/// ```
/// use ckb_resource::{Template, TemplateContext};
///
/// let template = Template::new(
///     r#"# # {{
/// ## _ => listen_address = "127.0.0.1:{rpc_port}"
/// ## }}"#
///         .to_string(),
/// );
/// let text = template.render(&TemplateContext::new("dev", vec![("rpc_port", "18114")]));
/// assert_eq!("listen_address = \"127.0.0.1:18114\"\n", text.unwrap());
/// ```
///
/// [`TemplateContext`]: struct.TemplateContext.html
/// [`TemplateContext::new`]: struct.TemplateContext.html#method_new
/// [`TemplateContext::insert`]: struct.TemplateContext.html#method_insert
pub struct Template(String);

/// The context used to expand the [`Template`](struct.Template.html).
pub struct TemplateContext<'a> {
    spec: &'a str,
    kvs: HashMap<&'a str, &'a str>,
}

impl<'a> TemplateContext<'a> {
    /// Creates a new template.
    ///
    /// * `spec` - the chain spec name for template spec branch.
    /// * `kvs` - the initial template variables.
    ///
    /// ## Examples
    ///
    /// ```
    /// use ckb_resource::TemplateContext;
    /// // Creates a context for *dev* chain and initializes variables:
    /// //
    /// //     rpc_port      => 8114
    /// //     p2p_port      => 8115
    /// TemplateContext::new("dev", vec![("rpc_port", "8114"), ("p2p_port", "8115")]);
    /// ```
    pub fn new<I>(spec: &'a str, kvs: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        Self {
            spec,
            kvs: kvs.into_iter().collect(),
        }
    }

    /// Inserts a new variable into the context.
    ///
    /// * `key` - the variable name
    /// * `value` - the variable value
    pub fn insert(&mut self, key: &'a str, value: &'a str) {
        self.kvs.insert(key, value);
    }
}

impl Template {
    /// Creates the template with the specified content.
    pub fn new(content: String) -> Self {
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

impl Template {
    /// Expands the template using the context and writes the result via the writer `w`.
    ///
    /// ## Errors
    ///
    /// This method returns `std::io::Error` when it fails to write the chunks to the underlying
    /// writer.
    pub fn render_to<'c, W: io::Write>(
        &self,
        w: &mut W,
        context: &TemplateContext<'c>,
    ) -> io::Result<()> {
        let spec_branch = format!("# {} => ", context.spec);

        let mut state = TemplateState::SearchStartMarker;
        for line in self.0.lines() {
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

    /// Renders the template and returns the result as a string.
    ///
    /// ## Errors
    ///
    /// This method returns `std::io::Error` when it fails to write the chunks to the underlying
    /// writer or it failed to convert the result text to UTF-8.
    pub fn render<'c>(&self, context: &TemplateContext<'c>) -> io::Result<String> {
        let mut out = Vec::new();
        self.render_to(&mut out, context)?;
        String::from_utf8(out)
            .map_err(|from_utf8_err| io::Error::new(io::ErrorKind::InvalidInput, from_utf8_err))
    }
}
