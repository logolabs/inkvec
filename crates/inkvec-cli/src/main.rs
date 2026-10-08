//! The `inkvec` command line binary.
fn main() -> inkvec_cli::ExitCode {
    inkvec_cli::cli_main()
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_main_unknown_arg_fails() {
        assert_eq!(
            inkvec_cli::cli_main_from(["--unknown-test-flag".to_string()].into_iter()),
            inkvec_cli::ExitCode::FAILURE
        );
    }
}
