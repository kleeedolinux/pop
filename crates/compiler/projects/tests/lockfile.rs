use pop_projects::{
    BubbleKind, BubbleLock, LockError, LockMode, LockedBubble, LockedBubbleIdentity, LockedPackage,
    LockedSource, apply_lock_policy, decode_lock, encode_lock, sha256_hex,
};

const ZERO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn package(name: &str, path: &str, features: &[&str]) -> LockedPackage {
    LockedPackage::new(
        name,
        "1.0.0",
        LockedSource::LocalPath(path.to_owned()),
        ZERO_HASH,
        features.iter().copied(),
    )
    .expect("locked Package")
}

fn lock(reversed: bool) -> BubbleLock {
    let mut packages = vec![
        package("Studio.Application", ".", &["logging", "metrics"]),
        package("Studio.Data", "packages/data", &[]),
    ];
    let mut bubbles = vec![
        LockedBubble::new(
            "Studio.Application",
            "Studio.Application",
            BubbleKind::Binary,
            [LockedBubbleIdentity::new(
                "Studio.Data",
                "Studio.Data",
                BubbleKind::Library,
            )],
        )
        .expect("application Bubble"),
        LockedBubble::new("Studio.Data", "Studio.Data", BubbleKind::Library, [])
            .expect("data Bubble"),
    ];
    if reversed {
        packages.reverse();
        bubbles.reverse();
    }
    BubbleLock::new("1", "x86_64-linux", packages, bubbles).expect("valid lock")
}

#[test]
fn canonical_lock_is_order_independent_and_round_trips_exact_bytes() {
    let first = encode_lock(&lock(false)).expect("canonical lock");
    let second = encode_lock(&lock(true)).expect("canonical lock from reversed inputs");
    assert_eq!(first, second);
    assert_eq!(first.last(), Some(&b'\n'));
    assert!(!first[..first.len() - 1].contains(&b'\n'));

    let decoded = decode_lock(&first).expect("verified canonical lock");
    assert_eq!(decoded, lock(false));
    assert_eq!(encode_lock(&decoded).expect("re-encode"), first);
    let text = String::from_utf8(first).expect("UTF-8 lock");
    assert!(text.starts_with(
        "{\"schemaVersion\":1,\"resolver\":\"1\",\"platformTarget\":\"x86_64-linux\""
    ));
    assert!(text.find("Studio.Application").unwrap() < text.find("Studio.Data").unwrap());
}

#[test]
fn malformed_noncanonical_and_cyclic_locks_fail_closed() {
    let canonical = encode_lock(&lock(false)).expect("canonical lock");
    let mut spaced = canonical.clone();
    spaced.insert(1, b' ');
    assert_eq!(decode_lock(&spaced), Err(LockError::NonCanonical));

    let unknown = String::from_utf8(canonical.clone())
        .expect("UTF-8")
        .replacen(
            "{\"schemaVersion\":1",
            "{\"unknown\":0,\"schemaVersion\":1",
            1,
        );
    assert_eq!(decode_lock(unknown.as_bytes()), Err(LockError::InvalidJson));

    let duplicate = String::from_utf8(canonical).expect("UTF-8").replacen(
        "{\"schemaVersion\":1",
        "{\"schemaVersion\":1,\"schemaVersion\":1",
        1,
    );
    assert_eq!(
        decode_lock(duplicate.as_bytes()),
        Err(LockError::InvalidJson)
    );

    let cycle = BubbleLock::new(
        "1",
        "x86_64-linux",
        [
            package("Studio.First", "first", &[]),
            package("Studio.Second", "second", &[]),
        ],
        [
            LockedBubble::new(
                "Studio.First",
                "Studio.First",
                BubbleKind::Library,
                [LockedBubbleIdentity::new(
                    "Studio.Second",
                    "Studio.Second",
                    BubbleKind::Library,
                )],
            )
            .expect("first"),
            LockedBubble::new(
                "Studio.Second",
                "Studio.Second",
                BubbleKind::Library,
                [LockedBubbleIdentity::new(
                    "Studio.First",
                    "Studio.First",
                    BubbleKind::Library,
                )],
            )
            .expect("second"),
        ],
    );
    assert_eq!(cycle, Err(LockError::DependencyCycle));
}

#[test]
fn locked_offline_and_frozen_modes_enforce_their_independent_rules() {
    let current = encode_lock(&lock(false)).expect("current lock");
    let changed = encode_lock(
        &BubbleLock::new(
            "1",
            "aarch64-linux",
            lock(false).packages().to_vec(),
            lock(false).bubbles().to_vec(),
        )
        .expect("changed lock"),
    )
    .expect("changed bytes");

    assert_eq!(
        apply_lock_policy(None, &current, LockMode::Locked, false),
        Err(LockError::MissingLock)
    );
    assert_eq!(
        apply_lock_policy(Some(&current), &changed, LockMode::Locked, false),
        Err(LockError::LockedChange)
    );
    assert_eq!(
        apply_lock_policy(Some(&current), &current, LockMode::Offline, true),
        Err(LockError::NetworkForbidden)
    );
    assert_eq!(
        apply_lock_policy(Some(&current), &changed, LockMode::Frozen, false),
        Err(LockError::LockedChange)
    );
    assert_eq!(
        apply_lock_policy(Some(&current), &current, LockMode::Frozen, true),
        Err(LockError::NetworkForbidden)
    );
    assert_eq!(
        apply_lock_policy(Some(&current), &current, LockMode::Frozen, false),
        Ok(false)
    );
    assert_eq!(
        apply_lock_policy(None, &current, LockMode::Normal, false),
        Ok(true)
    );
}

#[test]
fn sha256_has_the_standard_fixed_baseline() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
