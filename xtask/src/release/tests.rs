use super::*;

#[test]
fn sums_are_two_space_separated() {
    let sums = parse_sums("abc  one.tgz\ndef  two.json\n").unwrap();
    assert_eq!(sums, [("abc", "one.tgz"), ("def", "two.json")]);
    assert!(parse_sums("abc one\n").is_err());
}

#[test]
fn sums_list_every_asset_but_themselves() {
    let dir = env::temp_dir().join("xtask-voice-sums-test");
    empty_dir(&dir).unwrap();
    write(&dir.join(tarball_name("nightly")), b"tgz").unwrap();
    let sums = write_sums(&dir).unwrap();
    assert_eq!(
        sums,
        format!("{}  sidevoice-voice-nightly.tgz\n", sha256(b"tgz"))
    );
    assert_eq!(write_sums(&dir).unwrap(), sums);
}
