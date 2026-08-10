use fatwallet::wallet;

use solana_signer::Signer;

#[test]
fn test_keystore_encrypt_decrypt_roundtrip() {
    let keypair_bytes = [42u8; 64];
    let passphrase = "test_passphrase_123";
    let pubkey = "test_pubkey_abc";

    let keystore = wallet::keystore::encrypt_keypair(&keypair_bytes, passphrase, pubkey, "test")
        .expect("encrypt should succeed");

    assert_eq!(keystore.version, 1);
    assert_eq!(keystore.kdf, "argon2id");
    assert_eq!(keystore.cipher, "aes-256-gcm");
    assert_eq!(keystore.pubkey, pubkey);
    assert_eq!(keystore.label, "test");
    assert_eq!(keystore.kdf_params.salt.len(), 16);
    assert_eq!(keystore.nonce.len(), 12);
    assert!(!keystore.ciphertext.is_empty());

    let decrypted = wallet::keystore::decrypt_keypair(&keystore, passphrase)
        .expect("decrypt should succeed");

    assert_eq!(decrypted, keypair_bytes);
}

#[test]
fn test_keystore_wrong_passphrase_fails() {
    let keypair_bytes = [99u8; 64];
    let keystore = wallet::keystore::encrypt_keypair(&keypair_bytes, "correct", "pub", "label")
        .expect("encrypt should succeed");

    let result = wallet::keystore::decrypt_keypair(&keystore, "wrong");
    assert!(result.is_err());
}

#[test]
fn test_mnemonic_generation() {
    let mnemonic = wallet::keypair::generate_mnemonic().expect("mnemonic should generate");
    let words: Vec<&str> = mnemonic.split_whitespace().collect();
    assert_eq!(words.len(), 12, "should generate 12-word mnemonic");
}

#[test]
fn test_bip44_derivation_consistent() {
    let mnemonic = wallet::keypair::generate_mnemonic().expect("mnemonic should generate");

    let kp1 = wallet::keypair::keypair_from_seed_phrase(&mnemonic, "", 0, 0)
        .expect("derivation should succeed");
    let kp2 = wallet::keypair::keypair_from_seed_phrase(&mnemonic, "", 0, 0)
        .expect("derivation should succeed");

    assert_eq!(
        kp1.pubkey().to_string(),
        kp2.pubkey().to_string(),
        "same mnemonic + path should produce same keypair"
    );
}

#[test]
fn test_bip44_different_account_different_keypair() {
    let mnemonic = wallet::keypair::generate_mnemonic().expect("mnemonic should generate");

    let kp0 = wallet::keypair::keypair_from_seed_phrase(&mnemonic, "", 0, 0)
        .expect("derivation should succeed");
    let kp1 = wallet::keypair::keypair_from_seed_phrase(&mnemonic, "", 1, 0)
        .expect("derivation should succeed");

    assert_ne!(
        kp0.pubkey().to_string(),
        kp1.pubkey().to_string(),
        "different account should produce different keypair"
    );
}

#[test]
fn test_base58_import_roundtrip() {
    let mnemonic = wallet::keypair::generate_mnemonic().expect("mnemonic should generate");
    let kp = wallet::keypair::keypair_from_seed_phrase(&mnemonic, "", 0, 0)
        .expect("derivation should succeed");

    let key_bytes = kp.to_bytes();
    let base58 = bs58::encode(&key_bytes).into_string();

    let kp2 = wallet::keypair::keypair_from_base58(&base58)
        .expect("base58 import should succeed");

    assert_eq!(
        kp.pubkey().to_string(),
        kp2.pubkey().to_string(),
        "imported keypair should match original"
    );
}