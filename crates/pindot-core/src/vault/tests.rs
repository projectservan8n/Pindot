use super::*;
use keys::MemoryKeys;
use std::sync::atomic::Ordering;

const PW: &str = "Sup3r-Secret-Pw-9137";
const KEY32: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pindot-vault-{tag}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&d).unwrap();
    d
}

fn vault(tag: &str) -> (Vault, Arc<MemoryKeys>, PathBuf) {
    let dir = tmp(tag);
    let keys = Arc::new(MemoryKeys::default());
    (Vault::new(dir.clone(), keys.clone()), keys, dir)
}

fn login(name: &str, site: &str, user: &str) -> NewLogin {
    NewLogin { name: name.into(), site: site.into(), username: user.into(), password: PW.into(), authenticator: None }
}

#[test]
fn empty_vault_creates_no_key_until_something_is_saved() {
    let (v, keys, _) = vault("empty");
    assert!(v.list().unwrap().is_empty());
    assert!(keys.load().unwrap().is_none());
    v.add(login("A", "https://a.test", "ann")).unwrap();
    assert!(keys.load().unwrap().is_some());
}

#[test]
fn add_list_update_delete() {
    let (v, _, _) = vault("crud");
    let a = v.add(NewLogin { authenticator: Some(KEY32.into()), ..login("Work", "https://www.example.com/login", "ann") }).unwrap();
    assert_eq!(a.origin, "https://www.example.com");
    assert!(a.has_2fa && !a.locked);
    assert_eq!(a.username, "ann");
    v.add(login("Alpha", "https://alpha.test", "bob")).unwrap();
    let names: Vec<_> = v.list().unwrap().into_iter().map(|l| l.name).collect();
    assert_eq!(names, vec!["Alpha", "Work"]);

    // Edit without a new password keeps the old one, and moves the site.
    let e = v
        .update(
            &a.id,
            EditLogin {
                name: "Work 2".into(),
                site: "https://other.example.com".into(),
                username: "ann2".into(),
                password: None,
                authenticator: Change::Keep,
            },
        )
        .unwrap();
    assert_eq!((e.name.as_str(), e.origin.as_str(), e.username.as_str()), ("Work 2", "https://other.example.com", "ann2"));
    assert!(e.has_2fa);
    assert_eq!(&*v.secret(&a.id, Field::Password).unwrap(), PW);
    assert_eq!(&*v.secret(&a.id, Field::Username).unwrap(), "ann2");
    // Replace the password, clear the key.
    v.update(
        &a.id,
        EditLogin {
            name: "Work 2".into(),
            site: "https://other.example.com".into(),
            username: "ann2".into(),
            password: Some("new-pass".into()),
            authenticator: Change::Clear,
        },
    )
    .unwrap();
    assert_eq!(&*v.secret(&a.id, Field::Password).unwrap(), "new-pass");
    assert!(!v.get(&a.id).unwrap().has_2fa);
    assert!(v.secret(&a.id, Field::Code).is_err());

    v.delete(&a.id).unwrap();
    assert!(matches!(v.get(&a.id), Err(VaultError::NotFound)));
    assert!(matches!(v.delete(&a.id), Err(VaultError::NotFound)));
    assert_eq!(v.list().unwrap().len(), 1);
}

#[test]
fn bad_input_is_refused() {
    let (v, _, _) = vault("bad");
    assert!(v.add(login("", "https://a.test", "x")).is_err());
    assert!(v.add(login("A", "http://a.test", "x")).is_err());
    assert!(v.add(login("A", "https://x@a.test", "x")).is_err());
    assert!(v.add(NewLogin { password: String::new(), ..login("A", "https://a.test", "x") }).is_err());
    assert!(v.add(NewLogin { authenticator: Some("nope!!".into()), ..login("A", "https://a.test", "x") }).is_err());
    assert!(v.list().unwrap().is_empty());
}

#[test]
fn only_exact_origins_are_offered() {
    let (v, _, _) = vault("origin");
    v.add(login("Main", "https://example.com", "ann")).unwrap();
    v.add(login("Port", "https://example.com:8443", "ann")).unwrap();
    assert_eq!(v.for_origin("https://example.com/anything").unwrap().len(), 1);
    assert_eq!(v.for_origin("https://example.com:8443").unwrap()[0].name, "Port");
    assert!(v.for_origin("https://evil-example.com").unwrap().is_empty());
    assert!(v.for_origin("https://example.com.evil.test").unwrap().is_empty());
}

#[test]
fn nothing_secret_is_on_disk_in_the_clear() {
    let (v, _, dir) = vault("disk");
    v.add(NewLogin { authenticator: Some(KEY32.into()), ..login("Work", "https://example.com", "ann.private@example.test") }).unwrap();
    let raw = fs::read_to_string(dir.join(FILE)).unwrap();
    for needle in [PW, KEY32, "ann.private"] {
        assert!(!raw.contains(needle), "{needle} found in the vault file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(dir.join(FILE)).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn a_record_moved_to_another_site_or_id_cannot_be_opened() {
    let (v, _, dir) = vault("tamper");
    let a = v.add(login("A", "https://a.test", "ann")).unwrap();
    v.add(login("B", "https://b.test", "ann")).unwrap();
    let path = dir.join(FILE);
    let mut f: File = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    // Point A's saved login at another site: its values no longer open.
    for r in &mut f.logins {
        if r.id == a.id {
            r.origin = "https://evil.test".into();
        }
    }
    fs::write(&path, serde_json::to_string(&f).unwrap()).unwrap();
    assert!(matches!(v.secret(&a.id, Field::Password), Err(VaultError::CannotOpen)));
    let listed = v.list().unwrap();
    let a_now = listed.iter().find(|l| l.id == a.id).unwrap();
    assert!(a_now.locked && a_now.username.is_empty());
    // It can still be deleted.
    v.delete(&a.id).unwrap();
    // Swapping two records' password values fails too (bound to the id).
    let c = v.add(login("C", "https://b.test", "cat")).unwrap();
    let mut f: File = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let pw0 = f.logins[0].password.clone();
    f.logins[0].password = f.logins.iter().find(|r| r.id == c.id).unwrap().password.clone();
    for r in &mut f.logins {
        if r.id == c.id {
            r.password = pw0.clone();
        }
    }
    fs::write(&path, serde_json::to_string(&f).unwrap()).unwrap();
    assert!(v.secret(&c.id, Field::Password).is_err());
}

#[test]
fn fails_closed_when_the_key_is_missing_wrong_or_unreachable() {
    let (v, keys, dir) = vault("closed");
    v.add(login("A", "https://a.test", "ann")).unwrap();
    let before = fs::read_to_string(dir.join(FILE)).unwrap();

    // A fresh Vault (no cached key) with the key gone: nothing is created or written.
    let v2 = Vault::new(dir.clone(), keys.clone());
    keys.clear();
    assert!(matches!(v2.list(), Err(VaultError::KeyMissing)));
    assert!(matches!(v2.add(login("B", "https://b.test", "bob")), Err(VaultError::KeyMissing)));
    assert!(keys.load().unwrap().is_none(), "a new key must not replace the missing one");

    // A different key.
    keys.store(&Key::generate()).unwrap();
    let v3 = Vault::new(dir.clone(), keys.clone());
    assert!(matches!(v3.list(), Err(VaultError::WrongKey)));
    assert!(matches!(v3.add(login("B", "https://b.test", "bob")), Err(VaultError::WrongKey)));

    // The store cannot be reached.
    keys.fail.store(true, Ordering::SeqCst);
    let v4 = Vault::new(dir.clone(), keys.clone());
    assert!(matches!(v4.list(), Err(VaultError::KeyUnavailable(_))));
    assert_eq!(fs::read_to_string(dir.join(FILE)).unwrap(), before, "the file was never touched");
}

#[test]
fn totp_code_comes_from_the_saved_key() {
    let (v, _, _) = vault("code");
    let a = v.add(NewLogin { authenticator: Some(KEY32.into()), ..login("A", "https://a.test", "ann") }).unwrap();
    let code = v.secret(&a.id, Field::Code).unwrap();
    assert_eq!(code.len(), 6);
    assert!(code.chars().all(|c| c.is_ascii_digit()));
}

const CHROME: &str = "name,url,username,password,note\n\
Example,https://example.com/login,ann,pw-new-1,\n\
Second,https://second.test,bob,pw-new-2,\n\
NoPass,https://nopass.test,bob,,\n\
NoUrl,,bob,x,\n";

#[test]
fn import_preview_and_apply_with_counts() {
    let (v, _, _) = vault("import");
    let p = v.import_preview(CHROME).unwrap();
    assert_eq!((p.new_logins, p.unchanged, p.conflicts.len(), p.skipped.len()), (2, 0, 0, 2));
    let r = v.import_apply(CHROME, &[], "app").unwrap();
    assert_eq!((r.imported, r.skipped, r.replaced), (2, 2, 0));
    assert_eq!(v.list().unwrap().len(), 2);

    // Run it again: everything is already there with the same password.
    let p = v.import_preview(CHROME).unwrap();
    assert_eq!((p.new_logins, p.unchanged), (0, 2));
    let r = v.import_apply(CHROME, &[], "app").unwrap();
    assert_eq!((r.imported, r.unchanged, r.kept_existing), (0, 2, 0));
}

#[test]
fn import_never_overwrites_a_password_unless_asked() {
    let (v, _, _) = vault("conflict");
    let a = v.add(NewLogin { password: "mine-original".into(), ..login("Example", "https://example.com", "ann") }).unwrap();
    let p = v.import_preview(CHROME).unwrap();
    assert_eq!(p.conflicts.len(), 1);
    assert_eq!(p.conflicts[0].id, a.id);
    assert_eq!(p.new_logins, 1);

    // Not asked: kept.
    let r = v.import_apply(CHROME, &[], "app").unwrap();
    assert_eq!((r.imported, r.kept_existing, r.replaced), (1, 1, 0));
    assert_eq!(&*v.secret(&a.id, Field::Password).unwrap(), "mine-original");

    // Asked for that one: replaced.
    let r = v.import_apply(CHROME, std::slice::from_ref(&a.id), "app").unwrap();
    assert_eq!((r.replaced, r.kept_existing), (1, 0));
    assert_eq!(&*v.secret(&a.id, Field::Password).unwrap(), "pw-new-1");
    // An id that is not a conflict changes nothing.
    let other = v.list().unwrap().into_iter().find(|l| l.name == "Second").unwrap();
    v.import_apply(CHROME, std::slice::from_ref(&other.id), "app").unwrap();
    assert_eq!(&*v.secret(&other.id, Field::Password).unwrap(), "pw-new-2");
}

#[test]
fn import_adds_a_missing_authenticator_key_without_touching_the_password() {
    let (v, _, _) = vault("gainkey");
    let a = v.add(NewLogin { password: "keep-me".into(), ..login("Work", "https://work.example.test", "ann") }).unwrap();
    let csv = format!(
        "Title,Url,Username,Password,OTPAuth,Archived\nWork,https://work.example.test/x,ann,different,otpauth://totp/W:ann?secret={KEY32},false\n"
    );
    let p = v.import_preview(&csv).unwrap();
    assert_eq!((p.conflicts.len(), p.gain_key), (1, 1));
    let r = v.import_apply(&csv, &[], "app").unwrap();
    assert_eq!((r.keys_added, r.kept_existing, r.replaced), (1, 1, 0));
    assert!(v.get(&a.id).unwrap().has_2fa);
    assert_eq!(&*v.secret(&a.id, Field::Password).unwrap(), "keep-me");
}

#[test]
fn use_log_is_per_login_and_has_no_values() {
    let (v, _, dir) = vault("uses");
    let a = v.add(login("A", "https://a.test", "ann.visible@example.test")).unwrap();
    let b = v.add(login("B", "https://b.test", "bob")).unwrap();
    for (id, name, field, result, ok) in [
        (&a.id, "A", "password", "typed", true),
        (&a.id, "A", "password", "refused", false),
        (&b.id, "B", "username", "typed", true),
    ] {
        v.append_use(&UseEntry {
            ts: utc_now(),
            login_id: id.clone(),
            login_name: name.into(),
            site: "https://x.test".into(),
            field: field.into(),
            result: result.into(),
            detail: "reason".into(),
            session: "s1".into(),
            ok,
        });
    }
    let for_a = v.uses_for(&a.id, 50);
    // added + typed + refused
    assert_eq!(for_a.len(), 3);
    assert!(for_a.iter().all(|u| u.login_id == a.id));
    assert!(v.get(&a.id).unwrap().last_used.is_some());
    assert!(v.get(&b.id).unwrap().last_used.is_some());
    let raw = fs::read_to_string(dir.join(USES)).unwrap();
    assert!(!raw.contains(PW) && !raw.contains("ann.visible"));
}

/// Everything the app or the model can be handed, serialised, never holds
/// a password, a setup key, or the user name of a locked login.
#[test]
fn no_secret_appears_in_any_output() {
    let (v, _, dir) = vault("leak");
    let csv = format!("name,url,username,password\nImp,https://imp.test,imp.user.secret,{PW}-imp\n");
    let a = v.add(NewLogin { authenticator: Some(KEY32.into()), ..login("A", "https://a.test", "ann") }).unwrap();
    let mut outputs: Vec<String> = vec![
        serde_json::to_string(&v.list().unwrap()).unwrap(),
        serde_json::to_string(&v.get(&a.id).unwrap()).unwrap(),
        serde_json::to_string(&v.import_preview(&csv).unwrap()).unwrap(),
        serde_json::to_string(&v.import_apply(&csv, &[], "s").unwrap()).unwrap(),
        serde_json::to_string(&v.all_uses(100)).unwrap(),
        format!("{:?}", v.list().unwrap()),
    ];
    // Error texts.
    for e in [
        v.add(NewLogin { authenticator: Some(format!("{KEY32}!!")), ..login("X", "https://x.test", "u") }).unwrap_err(),
        v.import_preview("a,b\n1,2").unwrap_err(),
        VaultError::from(CryptoError::CannotOpen),
    ] {
        outputs.push(e.to_string());
        outputs.push(format!("{e:?}"));
    }
    outputs.push(fs::read_to_string(dir.join(USES)).unwrap());
    for out in &outputs {
        for needle in [PW, KEY32, "pw-new"] {
            assert!(!out.contains(needle), "secret {needle} leaked into: {out}");
        }
    }
}
