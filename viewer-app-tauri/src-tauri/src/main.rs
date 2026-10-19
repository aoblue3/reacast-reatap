// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
  // 管理者の権限で動く「キーの見張り係」としての起動(key_watcher.rs参照)であれば、
  // 見張りを行ってプロセスごと終了する(戻ってこない)。
  app_lib::key_watcher::run_if_requested();
  // アップデート差し替え役としての起動(updater.rs参照)であれば、ここで
  // 「待機→コピー→再起動」を全て行ってプロセスごと終了する(戻ってこない)。
  // 通常の起動であれば何もせずすぐ戻るので、いつも通りTauriアプリを始める。
  app_lib::updater::handle_apply_update_cli_if_present();
  app_lib::run();
}
