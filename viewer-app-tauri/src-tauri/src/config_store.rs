//! シンプルなJSON設定ファイルの読み書き。
//! Electron版のshared/json-store.jsと同じ考え方(getしたら既定値、
//! setしたら即ファイルに保存)をRust側で再現したもの。

use serde_json::{Map, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct ConfigStore {
    path: PathBuf,
    data: Mutex<Map<String, Value>>,
}

impl ConfigStore {
    pub fn load(path: PathBuf) -> Self {
        let data = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        Self {
            path,
            data: Mutex::new(data),
        }
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.data.lock().unwrap().get(key).cloned()
    }

    pub fn set(&self, key: String, value: Value) -> std::io::Result<()> {
        {
            let mut data = self.data.lock().unwrap();
            data.insert(key, value);
        }
        self.persist()
    }

    /// 設定の中身をまるごと取り出す(設定の書き出し用。backup.rs)
    pub fn snapshot(&self) -> Map<String, Value> {
        self.data.lock().unwrap().clone()
    }

    /// 設定の中身をまるごと入れ替えて保存する(設定の読み込み用。backup.rs)
    pub fn replace_all(&self, data: Map<String, Value>) -> std::io::Result<()> {
        *self.data.lock().unwrap() = data;
        self.persist()
    }

    fn persist(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = self.data.lock().unwrap();
        let json = serde_json::to_string_pretty(&Value::Object(data.clone()))?;
        fs::write(&self.path, json)
    }
}
