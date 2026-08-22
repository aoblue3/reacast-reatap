/// frontendディレクトリ以下の全ファイルを再帰的に辿り、cargoへ
/// 「このファイルが変わったらbuild.rsを再実行して」と1つずつ伝える。
///
/// 重要: `cargo:rerun-if-changed=<ディレクトリ>` を1回指定するだけでは
/// 不十分だった。cargoはディレクトリ自体のmtime(中のファイルが追加/削除
/// された時だけ更新される)しか見ておらず、既存ファイルの「内容の変更」
/// (既存のcontrol.js/control.htmlを編集しただけのケース)ではディレクトリ
/// 自体のmtimeは変化しないため、変更が無視されてしまっていた。そのため
/// 中のファイル1つ1つを個別に列挙してcargoへ渡す必要がある。
fn emit_rerun_if_changed_recursive(dir: &std::path::Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            emit_rerun_if_changed_recursive(&path);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

fn main() {
  // REACTION_RELAY_HOST/PORTはコンパイル時にoption_env!でexeへ焼き込む値なので、
  // 値が変わった時にcargoが再コンパイルするようこれを明示しておく必要がある
  // (指定しないと、ソースコードが変わっていない限りcargoが再ビルドをスキップし、
  // 古い値のまま埋め込まれ続けてしまうことがある)
  println!("cargo:rerun-if-env-changed=REACTION_RELAY_HOST");
  println!("cargo:rerun-if-env-changed=REACTION_RELAY_PORT");
  // frontendDist(tauri.conf.json)がビルド済みの成果物ではなく、../frontend配下の
  // 生のソースファイルを直接指しているため、cargoにとってはこれらのファイルは
  // 「ビルドに関係ない」ように見え、frontend/*.js・*.htmlだけを編集して
  // 再ビルドしても、他のRustソースが変わっていない限りbuild.rs自体が再実行
  // されず、Tauriが埋め込むアセットが古いまま(前回ビルド時点の内容のまま)に
  // なってしまう不具合があった(見た目には「ビルドは成功するのに変更が反映
  // されない」という形で現れる)。frontendディレクトリ配下のファイルを1つずつ
  // 明示的にビルドスクリプトの再実行トリガーに加えることで、frontendだけを
  // 編集した場合でも確実に最新の内容が埋め込まれるようにする。
  emit_rerun_if_changed_recursive(std::path::Path::new("../frontend"));
  tauri_build::build()
}
