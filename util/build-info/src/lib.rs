// some code taken and adapted from RLS and cargo
#[derive(Debug, Default, Clone)]
pub struct Version {
    pub major: u8,
    pub minor: u8,
    pub patch: u16,
    pub dash_pre: String,
    pub code_name: Option<String>,
    pub commit_describe: Option<String>,
    pub commit_date: Option<String>,
}

impl Version {
    pub fn short(&self) -> String {
        format!(
            "{}.{}.{}{}",
            self.major, self.minor, self.patch, self.dash_pre
        )
    }

    pub fn long(&self) -> String {
        self.to_string()
    }

    pub fn is_pre(&self) -> bool {
        self.dash_pre != ""
    }

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
