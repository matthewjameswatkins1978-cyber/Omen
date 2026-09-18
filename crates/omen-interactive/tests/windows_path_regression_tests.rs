use omen_interactive::grammar::{GrammarScanner, InputLane};

#[test]
fn test_windows_drive_paths() {
    let input = r#"cargo test C:\Users\Matmus\project"#;
    let lane = GrammarScanner::scan(input).unwrap();
    assert_eq!(
        lane,
        InputLane::Executable {
            argv: vec![
                "cargo".into(),
                "test".into(),
                r#"C:\Users\Matmus\project"#.into(),
            ]
        }
    );

    let input_d = r#"notepad D:\Omen Shell\crates\omen-interactive\src\lib.rs"#;
    let lane_d = GrammarScanner::scan(input_d).unwrap();
    assert_eq!(
        lane_d,
        InputLane::Executable {
            argv: vec![
                "notepad".into(),
                "D:\\Omen".into(),
                "Shell\\crates\\omen-interactive\\src\\lib.rs".into(),
            ]
        }
    );
}

#[test]
fn test_windows_unc_paths() {
    let input = r#"dir \\server\share\data\output.json"#;
    let lane = GrammarScanner::scan(input).unwrap();
    assert_eq!(
        lane,
        InputLane::Executable {
            argv: vec!["dir".into(), r#"\\server\share\data\output.json"#.into(),]
        }
    );
}

#[test]
fn test_windows_quoted_paths_with_spaces() {
    let input = r#"exec "C:\Program Files\Rust\bin\cargo.exe" --version"#;
    let lane = GrammarScanner::scan(input).unwrap();
    assert_eq!(
        lane,
        InputLane::Executable {
            argv: vec![
                "exec".into(),
                r#"C:\Program Files\Rust\bin\cargo.exe"#.into(),
                "--version".into(),
            ]
        }
    );

    let input_single = r#"run 'D:\My Projects\app.exe' arg1"#;
    let lane_single = GrammarScanner::scan(input_single).unwrap();
    assert_eq!(
        lane_single,
        InputLane::Executable {
            argv: vec![
                "run".into(),
                r#"D:\My Projects\app.exe"#.into(),
                "arg1".into(),
            ]
        }
    );
}

#[test]
fn test_literal_and_consecutive_backslashes() {
    let input = r#"echo foo\\bar and \\\more"#;
    let lane = GrammarScanner::scan(input).unwrap();
    assert_eq!(
        lane,
        InputLane::Executable {
            argv: vec![
                "echo".into(),
                r#"foo\\bar"#.into(),
                "and".into(),
                r#"\\\more"#.into(),
            ]
        }
    );
}

#[test]
fn test_trailing_backslashes() {
    let input = r#"dir C:\test\ and D:\other\"#;
    let lane = GrammarScanner::scan(input).unwrap();
    assert_eq!(
        lane,
        InputLane::Executable {
            argv: vec![
                "dir".into(),
                r#"C:\test\"#.into(),
                "and".into(),
                r#"D:\other\"#.into(),
            ]
        }
    );
}

#[test]
fn test_malformed_and_incomplete_quoting() {
    let input = r#"open "C:\unclosed quote\path"#;
    let lane = GrammarScanner::scan(input).unwrap();
    assert_eq!(
        lane,
        InputLane::Executable {
            argv: vec!["open".into(), r#"C:\unclosed quote\path"#.into(),]
        }
    );

    let input_escaped_quote = r#"echo "say \"hello\" to world""#;
    let lane_escaped = GrammarScanner::scan(input_escaped_quote).unwrap();
    assert_eq!(
        lane_escaped,
        InputLane::Executable {
            argv: vec!["echo".into(), r#"say "hello" to world"#.into(),]
        }
    );
}
