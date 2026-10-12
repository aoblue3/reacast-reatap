//! 読み上げの「スキップ」「一時停止/再開」のホットキー(OS全体で効くグローバル
//! ショートカット)。設定画面を閉じていても効くように、Rust側で登録する。
//! キーの書き方はReaTapと同じ("Ctrl+Alt+KeyS"のように、修飾キー+キーのコード)。

use super::{CommentEngine, CommentSettings};
use std::sync::Arc;
use tauri::{plugin::TauriPlugin, AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

fn parse(key: &str) -> Option<Shortcut> {
    let key = key.trim();
    if key.is_empty() {
        None
    } else {
        key.parse().ok()
    }
}

pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            let Some(engine) = app.try_state::<Arc<CommentEngine>>() else {
                return;
            };
            let s = engine.settings();
            if parse(&s.hotkey_skip).map(|k| k.id()) == Some(shortcut.id()) {
                engine.skip();
            } else if parse(&s.hotkey_pause).map(|k| k.id()) == Some(shortcut.id()) {
                engine.toggle_pause();
            }
        })
        .build()
}

/// 設定のホットキーを登録し直す。登録できなかったものがあれば、その説明を返す。
pub fn apply(app: &AppHandle, s: &CommentSettings) -> Result<(), String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let mut errors = Vec::new();
    for (label, key) in [("スキップ", &s.hotkey_skip), ("一時停止/再開", &s.hotkey_pause)] {
        if key.trim().is_empty() {
            continue;
        }
        match parse(key) {
            Some(k) => {
                if gs.register(k).is_err() {
                    errors.push(format!("{label}のキー「{key}」を登録できませんでした(他のアプリで使われている可能性があります)"));
                }
            }
            None => errors.push(format!("{label}のキー「{key}」の書き方が正しくありません")),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

/// 設定画面でキーを割り当てている間は、押したキーがホットキーとして奪われない
/// ように一旦すべて外す(割り当てが終わったらapplyで戻す)。
pub fn suspend(app: &AppHandle) {
    let _ = app.global_shortcut().unregister_all();
}
