use arch_install_manager::engine::colorize_shell;

#[test]
fn colorize_shell_highlights_comments_and_strings() {
    let plain = colorize_shell("# c\nname='x'\n", false);
    assert_eq!(plain, "# c\nname='x'\n");

    let colored = colorize_shell("# c\nname='x'\necho \"hi\"\n", true);
    assert!(colored.contains("\x1b[90m# c\x1b[0m"));
    assert!(colored.contains("\x1b[32m'x'\x1b[0m"));
    assert!(colored.contains("\x1b[32m\"hi\"\x1b[0m"));
    assert!(colored.ends_with('\n'));
}
