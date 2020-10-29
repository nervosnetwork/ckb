//! The crate `ckb-build-info` generates CKB version from the build environment.

/// CKB version
#[derive(Debug, Default, Clone)]
pub struct Version {
    /// The major version.
    ///
    /// It is the x in `x.y.z`.
    pub major: u8,
    /// The minor version.
    ///
    /// It is the y in `x.y.z`.
    pub minor: u8,
    /// The patch version.
    ///
    /// It is the z in `x.y.z`.
    pub patch: u16,
    /// The pre-release version.
    ///
    /// It is the part starting with `-`.
    ///
    /// ## Examples
    ///
    /// * `v1.2.3`: `dash_pre` is ""
    /// * `v1.2.3-rc1`: `dash_pre` is "-rc1"
    pub dash_pre: String,
    /// A nickname of the version.
    pub code_name: Option<String>,
    /// The SHA of the last Git commit.
    ///
    /// See [`get_commit_describe`] how to get it.
    ///
    /// [`get_commit_describe`]: fn.get_commit_describe.html
    pub commit_describe: Option<String>,
    /// The commit date of the last Git commit.
    ///
    /// See [`get_commit_date`] how to get it.
    ///
    /// [`get_commit_date`]: fn.get_commit_date.html
    pub commit_date: Option<String>,
}

impl Version {
    /// Returns short representation of the version.
    ///
    /// It returns version in format like `x.y.z` or `x.y.z-pre`.
    pub fn short(&self) -> String {
        format!(
            "{}.{}.{}{}",
            self.major, self.minor, self.patch, self.dash_pre
        )
    }

    /// Returns full representation of the version.
    ///
    /// It adds extra information after the short version in parenthesis, for example:
    ///
    /// `0.36.0 (7692751 2020-09-21)`
    pub fn long(&self) -> String {
        self.to_string()
    }

    /// Tells whether this is a pre-release version.
    pub fn is_pre(&self) -> bool {
        self.dash_pre != ""
    }

    /// Tells whether this version is build from a dirty git working directory.
    ///
    /// The dirty version is built from the source code which has uncommitted changes.
    pub fn is_dirty(&self) -> bool {
        if let Some(describe) = &self.commit_describe {
            describe.ends_with("-dirty")
        } else {
            false
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.{}.{}{}",
            self.major, self.minor, self.patch, self.dash_pre
        )?;

        let extra_parts: Vec<_> = self
            .code_name
            .iter()
            .chain(self.commit_describe.iter())
            .chain(self.commit_date.iter())
            .map(String::as_str)
            .collect();
        if !extra_parts.is_empty() {
            write!(f, " ({})", extra_parts.as_slice().join(" "))?;
        }

        Ok(())
    }
}

/// Gets the field [`commit_describe`] via Git.
///
/// [`commit_describe`]: struct.Version.html#structfield.commit_describe
pub fn get_commit_describe() -> Option<String> {
    std::process::Command::new("git")
        .args(&[
            "describe",
            "--dirty",
            "--always",
            "--match",
            "__EXCLUDE__",
            "--abbrev=7",
        ])
        .output()
        .ok()
        .and_then(|r| {
            String::from_utf8(r.stdout)
                .ok()
                .map(|s| s.trim().to_string())
        })
}

/// Gets the field [`commit_date`] via Git.
///
/// [`commit_date`]: struct.Version.html#structfield.commit_date
pub fn get_commit_date() -> Option<String> {
    std::process::Command::new("git")
        .env("TZ", "UTC")
        .args(&["log", "-1", "--date=short-local", "--pretty=format:%cd"])
        .output()
        .ok()
        .and_then(|r| {
            String::from_utf8(r.stdout)
                .ok()
                .map(|s| s.trim().to_string())
        })
}
