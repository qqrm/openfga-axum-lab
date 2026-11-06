//! Secret values may come from the environment or a mounted file, never both.
use std::{env, io, path::Path};

pub fn secret(name: &str) -> io::Result<String> {
    let direct = env::var_os(name);
    let file = env::var_os(format!("{name}_FILE"));
    resolve(name, direct.as_deref(), file.as_deref())
}

fn resolve(
    name: &str,
    value: Option<&std::ffi::OsStr>,
    file: Option<&std::ffi::OsStr>,
) -> io::Result<String> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("set exactly one of {name} and {name}_FILE"),
        )
    };
    let value = match (value, file) {
        (Some(value), None) => value.to_str().ok_or_else(invalid)?.to_owned(),
        (None, Some(file)) => std::fs::read_to_string(Path::new(file))?
            .trim_end_matches(['\r', '\n'])
            .to_owned(),
        _ => return Err(invalid()),
    };
    if value.is_empty() {
        return Err(invalid());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use std::ffi::OsStr;
    #[test]
    fn ambiguity_and_missing_values_fail_closed() {
        assert!(resolve("TEST", None, None).is_err());
        assert!(resolve("TEST", Some(OsStr::new("value")), Some(OsStr::new("file"))).is_err());
        assert!(resolve("TEST", Some(OsStr::new("")), None).is_err());
    }
    #[test]
    fn mounted_secret_preserves_spaces_but_not_line_endings() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("secret");
        std::fs::write(&file, " value \r\n").unwrap();
        assert_eq!(
            resolve("TEST", None, Some(file.as_os_str())).unwrap(),
            " value "
        );
    }
}
