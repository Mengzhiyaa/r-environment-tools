/// Quote one argument for a POSIX-compatible shell command.
///
/// Arguments containing only portable, non-expanding characters are returned
/// unchanged so existing startup commands remain readable. All other arguments
/// are single-quoted, with embedded single quotes escaped by ending and
/// restarting the quoted string.
pub fn quote_shell_argument(argument: &str) -> String {
    if !argument.is_empty()
        && argument
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
    {
        return argument.to_string();
    }

    format!("'{}'", argument.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::quote_shell_argument;

    #[test]
    fn leaves_safe_arguments_unquoted() {
        assert_eq!(
            quote_shell_argument("/opt/conda/envs/r-prod"),
            "/opt/conda/envs/r-prod"
        );
    }

    #[test]
    fn quotes_spaces_and_single_quotes() {
        assert_eq!(
            quote_shell_argument("/opt/conda/envs/r prod's"),
            "'/opt/conda/envs/r prod'\"'\"'s'"
        );
    }

    #[cfg(unix)]
    #[test]
    fn quoted_arguments_round_trip_through_a_shell() {
        for argument in [
            "",
            "R project",
            "R user's",
            "$(printf expanded)",
            "`printf expanded`",
            "a;b",
            "line\nbreak",
        ] {
            let script = format!("printf %s {}", quote_shell_argument(argument));
            let output = std::process::Command::new("sh")
                .args(["-c", &script])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(String::from_utf8(output.stdout).unwrap(), argument);
        }
    }
}
