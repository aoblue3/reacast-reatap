//! 部屋の永続的な識別情報(roomId・broadcasterToken)と、初期表示用の
//! 合言葉(passphrase)のランダム生成。
//!
//! 以前はここで接続コード全体(中継サーバーのアドレス・部屋ID・視聴者トークンを
//! AES-256-GCMで暗号化してbase32化したもの)を発行していたが、コードが長く
//! 手入力しづらいという要望を受けて、視聴者は「配信者が決めた短い合言葉」を
//! 中継サーバーに直接送って参加する方式に変更した。中継サーバーのアドレスは
//! 視聴者アプリのビルド時に埋め込む(get_relay_addressコマンド参照)ようにしたので、
//! 接続コードとして別途配る必要が無くなった。そのため暗号化まわりのコードは
//! 不要になり削除した(roomId/broadcasterTokenの生成だけが引き続き必要)。

use rand::RngCore;

// 合言葉の初期値生成に使う文字セット。紛らわしい文字(0/o, 1/l/i等)を除いた
// 小文字英数字のみにして、口頭やチャットで伝えても読み間違えにくくしてある。
const PASSPHRASE_ALPHABET: &[u8] = b"abcdefghjkmnpqrstvwxyz23456789";
const DEFAULT_PASSPHRASE_LEN: usize = 8;

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// ランダムな部屋ID・配信者トークンを生成する(すべてhex文字列)。
///
/// roomIdは中継サーバー内部での部屋の識別にのみ使う(以前と違い、視聴者が
/// 直接目にすることはない)。broadcasterTokenは、この部屋への再登録
/// (配信者アプリの再起動・再接続)を他人に乗っ取られないようにするための
/// 秘密情報で、配信者アプリの外に出すことはない。
pub fn generate_room_credentials() -> (String, String) {
    let mut rng = rand::thread_rng();

    let mut room_id_buf = [0u8; 5];
    rng.fill_bytes(&mut room_id_buf);

    let mut broadcaster_token_buf = [0u8; 24];
    rng.fill_bytes(&mut broadcaster_token_buf);

    (
        hex_encode(&room_id_buf),
        hex_encode(&broadcaster_token_buf),
    )
}

/// 初回起動時、まだ何も設定していない配信者にもすぐ使える合言葉を用意しておく
/// ための初期値生成(あくまで初期値。配信者は設定パネルからいつでも好きな
/// 文字列に変更できる)。
pub fn generate_default_passphrase() -> String {
    let mut rng = rand::thread_rng();
    (0..DEFAULT_PASSPHRASE_LEN)
        .map(|_| {
            let idx = (rng.next_u32() as usize) % PASSPHRASE_ALPHABET.len();
            PASSPHRASE_ALPHABET[idx] as char
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_have_expected_lengths() {
        let (room_id, broadcaster_token) = generate_room_credentials();
        assert_eq!(room_id.len(), 10); // 5 byte -> 10 hex chars
        assert_eq!(broadcaster_token.len(), 48); // 24 byte -> 48 hex chars
    }

    #[test]
    fn generated_ids_are_hex() {
        let (room_id, broadcaster_token) = generate_room_credentials();
        assert!(room_id.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(broadcaster_token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn default_passphrase_has_expected_length_and_charset() {
        let p = generate_default_passphrase();
        assert_eq!(p.chars().count(), DEFAULT_PASSPHRASE_LEN);
        assert!(p.bytes().all(|b| PASSPHRASE_ALPHABET.contains(&b)));
    }

    #[test]
    fn default_passphrase_is_reasonably_random() {
        // 2回生成して毎回同じにならないことをざっくり確認する(完全な乱数性の
        // 証明ではないが、定数を返すような明らかな実装ミスは検出できる)
        let a = generate_default_passphrase();
        let b = generate_default_passphrase();
        assert_ne!(a, b);
    }
}
