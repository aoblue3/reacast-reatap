"""CHANGELOG.md から指定バージョンの節を取り出し、GitHub Releaseのタイトルと本文を作る。

タイトルは見出しそのまま(例: "v1.5.3: バージョン表示・不具合調査用のログを追加")。
本文は、アプリの自動アップデートの確認ダイアログにそのまま出るので、Markdownを
使わない短いテキストにする(項目の太字部分だけを「・」で並べる)。
v1.5.3以前のアプリはダイアログに高さの上限が無いため、長くしすぎないこと。

使い方: python release_notes.py <タグ名> <本文の出力先>
タイトルは GITHUB_OUTPUT に title=... として書く(無ければ標準出力)。
"""
import os
import re
import sys

MAX_ITEMS = 12


def extract(changelog: str, tag: str):
    lines = changelog.splitlines()
    start = None
    for i, line in enumerate(lines):
        if re.match(rf"^## {re.escape(tag)}(:|\s*$)", line):
            start = i
            break
    if start is None:
        return tag, ""
    title = lines[start][3:].strip()
    section = []
    for line in lines[start + 1:]:
        if line.startswith("## "):
            break
        section.append(line)

    out = []
    items = 0
    for line in section:
        if line.startswith("### "):
            out.append(f"【{line[4:].strip()}】")
        elif line.startswith("- "):
            items += 1
            if items > MAX_ITEMS:
                continue
            text = line[2:].strip()
            m = re.match(r"\*\*(.+?)\*\*", text)
            if m:
                text = m.group(1).rstrip(":：")
            else:
                text = text.replace("**", "")
                # 太字の見出しが無い項目は、最初の文だけにする
                end = text.find("。")
                if end != -1:
                    text = text[: end + 1]
            out.append(f"・{text}")
    if items > MAX_ITEMS:
        out.append(f"・ほか{items - MAX_ITEMS}件")
    # 見出しの直後に項目が無い(注意書きだけ等の)節は見出しごと落とす
    cleaned = [l for i, l in enumerate(out) if not (l.startswith("【") and (i + 1 == len(out) or out[i + 1].startswith("【")))]
    return title, "\n".join(cleaned)


def main():
    tag, body_path = sys.argv[1], sys.argv[2]
    with open(os.path.join(os.path.dirname(__file__), "..", "..", "CHANGELOG.md"), encoding="utf-8") as f:
        title, body = extract(f.read(), tag)
    with open(body_path, "w", encoding="utf-8") as f:
        f.write(body + "\n")
    gh_out = os.environ.get("GITHUB_OUTPUT")
    if gh_out:
        with open(gh_out, "a", encoding="utf-8") as f:
            f.write(f"title={title}\n")
    else:
        sys.stdout.reconfigure(encoding="utf-8")
        print(title)


if __name__ == "__main__":
    main()
