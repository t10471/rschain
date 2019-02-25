use rand::rngs::OsRng;
use sha2::Sha512;
use ed25519_dalek::{Keypair, SecretKey, PublicKey};
use rust_base58::ToBase58;
use rocksdb::DB;
use std::fs;
use std::path::Path;
use dirs;

const ACCOUNT_DIR: &'static str = ".rschain";

type Address = String;

pub struct Account {
  pub secret: Option<SecretKey>,
  pub public: PublicKey,
}

impl Account {

  pub fn address(&self) -> Address {
    self.public.to_bytes().to_base58()
  }
}

pub fn create() -> Account {
  let mut cspring: OsRng = OsRng::new().unwrap();
  let k: Keypair = Keypair::generate::<Sha512, _>(&mut cspring);
  let account = Account {secret: Some(k.secret), public: k.public};
  save_account(&account);
  account
}

fn get_account_dir() -> String {
  let mut h = dirs::home_dir().unwrap();
  h.push(ACCOUNT_DIR);
  h.into_os_string().into_string().unwrap()
}

fn save_account(account: &Account) {
  mkdir();
  let db = DB::open_default(get_account_dir()).unwrap();
  account.secret.as_ref().map(|s| db.put(account.address().as_bytes(), s.as_bytes()));
}

fn mkdir() {
  if !Path::new(get_account_dir().as_str()).exists() {
    fs::create_dir(get_account_dir()).unwrap();
  }
}
