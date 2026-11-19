use super::*;
use crate::commands::doctor::{self, Status};
use crate::json;
use crate::updater::Config;

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bunny-signer-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_pair_is_two_lines_of_base64_that_sign_and_verify() {
    let dir = folder("pair");
    let key = SigningKey::from_seed(&[11; 32]);
    let pair = write_pair(&key, &dir.join("keys"), "updater", false).unwrap();
    let public = fs::read_to_string(&pair.public).unwrap();
    let private = fs::read_to_string(&pair.private).unwrap();
    assert!(public.ends_with('\n') && private.ends_with('\n'));
    let verifier = updater::public_key(&public).unwrap();
    let signer = updater::signing_key(&private).unwrap();
    assert_eq!(signer.seed(), key.seed());
    let signature = signer.sign(b"the package");
    assert!(verifier.verify(b"the package", &signature));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&pair.private).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_pair_that_is_there_stays_without_force() {
    let dir = folder("force");
    let first = SigningKey::from_seed(&[1; 32]);
    let pair = write_pair(&first, &dir, "updater", false).unwrap();
    let before = fs::read_to_string(&pair.private).unwrap();
    let error = write_pair(&SigningKey::from_seed(&[2; 32]), &dir, "updater", false).unwrap_err();
    assert!(error.message.contains("is there already"), "{}", error.message);
    assert_eq!(fs::read_to_string(&pair.private).unwrap(), before, "nothing is written");
    // one file of the two is enough to refuse
    fs::remove_file(&pair.private).unwrap();
    assert!(write_pair(&first, &dir, "updater", false).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(&pair.private, "old").unwrap();
        fs::set_permissions(&pair.private, fs::Permissions::from_mode(0o644)).unwrap();
    }
    write_pair(&SigningKey::from_seed(&[2; 32]), &dir, "updater", true).unwrap();
    assert_ne!(fs::read_to_string(&pair.private).unwrap(), before);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&pair.private).unwrap().permissions().mode() & 0o777, 0o600, "--force makes it secret again");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_private_key_goes_into_gitignore_once() {
    let dir = folder("ignore");
    fs::write(dir.join(".gitignore"), "/target").unwrap();
    let private = dir.join("keys").join("updater.key");
    assert_eq!(ignore(&dir, &private).unwrap(), Ignored::Added(String::from("/keys/updater.key")));
    assert_eq!(ignore(&dir, &private).unwrap(), Ignored::Already);
    assert_eq!(fs::read_to_string(dir.join(".gitignore")).unwrap(), "/target\n# the updater's private key\n/keys/updater.key\n");
    // a `*.key` line covers every key
    fs::write(dir.join(".gitignore"), "*.key\n").unwrap();
    assert_eq!(ignore(&dir, &dir.join("backup.key")).unwrap(), Ignored::Already);
    assert_eq!(ignore(&dir, Path::new("/elsewhere/updater.key")).unwrap(), Ignored::Outside);
    // no .gitignore yet
    let bare = folder("ignore-bare");
    assert_eq!(ignore(&bare, &bare.join("k.key")).unwrap(), Ignored::Added(String::from("/k.key")));
    assert!(fs::read_to_string(bare.join(".gitignore")).unwrap().ends_with("/k.key\n"));
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&bare);
}

#[test]
fn the_snippet_is_a_table_the_cli_reads() {
    for (product, package) in [(None, "notes"), (Some("photo-lab"), "photo_lab")] {
        let text = snippet("keys/updater.pub", product, product.unwrap_or(package));
        // the snippet as `cargo metadata` would give it
        let mut members = Vec::new();
        for line in text.lines().skip(1) {
            let (key, value) = line.split_once(" = ").unwrap();
            members.push(format!("{}:{}", json::quote(key), value));
        }
        let table = json::parse(&format!("{{{}}}", members.join(","))).unwrap();
        let config = Config::parse(&table, package, Path::new("/app")).unwrap();
        assert_eq!(config.product, product.unwrap_or(package));
        assert_eq!(config.pubkeys, ["keys/updater.pub"]);
        assert!(config.url.unwrap().contains("{file}"));
    }
}

#[test]
fn doctor_warns_about_a_key_outside_git() {
    let dir = folder("doctor");
    let key = SigningKey::from_seed(&[5; 32]);
    write_pair(&key, &dir.join("keys"), "updater", false).unwrap();
    fs::write(dir.join("keys/broken.pub"), "AAAA\n").unwrap();
    let table = json::parse(r#"{"feed":"https://a","pubkeys":["keys/updater.pub","keys/broken.pub","keys/gone.pub"]}"#).unwrap();
    let config = Config::parse(&table, "app", &dir).unwrap();
    let checks = doctor::updater_checks(&config, &dir, &|path| Some(path != Path::new("keys/updater.pub")));
    let statuses: Vec<Status> = checks.iter().map(|check| check.status).collect();
    assert_eq!(statuses, [Status::Warn, Status::Fail, Status::Fail]);
    assert!(checks[0].title.contains("keys/updater.pub is not in git"), "{}", checks[0].title);
    assert_eq!(checks[0].fixes, ["git add keys/updater.pub && git commit"]);
    let tracked = doctor::updater_checks(&config, &dir, &|_| Some(true));
    assert_eq!(tracked[0].status, Status::Ok);
    let no_git = doctor::updater_checks(&config, &dir, &|_| None);
    assert_eq!(no_git[0].status, Status::Ok);
    let _ = fs::remove_dir_all(&dir);
}
