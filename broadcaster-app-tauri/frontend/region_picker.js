'use strict';
/**
 * 「範囲指定」ボタン(control.js)から開かれる、表示範囲をドラッグで選ぶ
 * 専用ウィンドウ。このウィンドウ自体が選んだモニターの外枠ぴったりの
 * サイズ・位置で開かれる(lib.rsのopen_region_picker参照)ため、
 * window.innerWidth/innerHeightがそのままモニターの物理サイズに対応し、
 * ドラッグした範囲をそのままモニターに対する割合(%)に変換できる。
 *
 * 確定(mouseup)したら、このファイル自身がset_overlay_regionを直接呼んで
 * 保存・オーバーレイへの反映まで済ませ、コントロールパネル側へは
 * "overlay-region-picked"イベントでその値を知らせてスライダーの表示だけ
 * 同期してもらう(実際の保存・反映はここで完結させておくことで、
 * コントロールパネルが閉じられていても問題なく機能する)。
 */
const invoke = window.__TAURI__.core.invoke;
const emit = window.__TAURI__.event.emit;
const getCurrentWindow = window.__TAURI__.window.getCurrentWindow;

// このウィンドウには右クリックで使う要素が無いため、素のコンテキストメニューを
// 抑止する(bar.js等、他の枠なしウィンドウと同じ理由)。
window.addEventListener('contextmenu', (e) => e.preventDefault());

const selectionBoxEl = document.getElementById('selectionBox');
const selectionLabelEl = document.getElementById('selectionLabel');
const cancelBtn = document.getElementById('cancelBtn');
const hintEl = document.getElementById('hint');

// ドラッグが小さすぎる(誤ってクリックしただけ等)場合は確定させず、
// やり直せるようにする(モニター短辺に対する割合で判定)。
const MIN_DRAG_RATIO = 0.02;

let dragging = false;
let startX = 0;
let startY = 0;

function closeSelf() {
  getCurrentWindow()
    .close()
    .catch(() => {});
}

function currentRect(clientX, clientY) {
  const left = Math.min(startX, clientX);
  const top = Math.min(startY, clientY);
  const width = Math.abs(clientX - startX);
  const height = Math.abs(clientY - startY);
  return { left, top, width, height };
}

function renderSelection(rect) {
  selectionBoxEl.style.display = 'block';
  selectionBoxEl.style.left = `${rect.left}px`;
  selectionBoxEl.style.top = `${rect.top}px`;
  selectionBoxEl.style.width = `${rect.width}px`;
  selectionBoxEl.style.height = `${rect.height}px`;
  const wPercent = Math.round((rect.width / window.innerWidth) * 100);
  const hPercent = Math.round((rect.height / window.innerHeight) * 100);
  selectionLabelEl.textContent = `${wPercent}% × ${hPercent}%`;
}

window.addEventListener('mousedown', (e) => {
  if (e.button !== 0) return;
  if (e.target.closest('#cancelBtn')) return;
  dragging = true;
  startX = e.clientX;
  startY = e.clientY;
  hintEl.style.display = 'none';
  renderSelection(currentRect(e.clientX, e.clientY));
});

window.addEventListener('mousemove', (e) => {
  if (!dragging) return;
  renderSelection(currentRect(e.clientX, e.clientY));
});

window.addEventListener('mouseup', (e) => {
  if (!dragging) return;
  dragging = false;
  const rect = currentRect(e.clientX, e.clientY);
  const minSize = Math.min(window.innerWidth, window.innerHeight) * MIN_DRAG_RATIO;
  if (rect.width < minSize || rect.height < minSize) {
    // ドラッグが小さすぎる = 誤クリックとみなし、確定させずにやり直せるようにする
    selectionBoxEl.style.display = 'none';
    hintEl.textContent = 'ドラッグの範囲が小さすぎます。もう一度、左クリックを押したままドラッグしてください(Escでキャンセル)';
    hintEl.style.display = 'block';
    return;
  }

  const x = rect.left / window.innerWidth;
  const y = rect.top / window.innerHeight;
  const width = rect.width / window.innerWidth;
  const height = rect.height / window.innerHeight;

  invoke('set_overlay_region', { enabled: true, x, y, width, height })
    .catch(() => {
      // 無視(保存に失敗しても、この画面を開いたままにする実害の方が大きいので閉じる)
    })
    .finally(() => {
      // コントロールパネルが開いていれば、スライダー表示をこの値に同期させる
      // (control.jsのlisten('overlay-region-picked', ...)参照)。ブロードキャスト
      // なので他のウィンドウ(オーバーレイ等)には単に無視される。
      emit('overlay-region-picked', { x, y, width, height }).catch(() => {});
      closeSelf();
    });
});

window.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    closeSelf();
  }
});

cancelBtn.addEventListener('click', () => {
  closeSelf();
});
