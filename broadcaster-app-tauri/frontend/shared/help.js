// 設定画面の「ⓘ」。<span class="help" data-help="詳しい説明"></span> と書くと、押した時だけ
// 吹き出しで説明を出す(普段は1行の短い説明だけ見せて、画面を文字だらけにしないため)。
// 吹き出しは画面に固定(position: fixed)で出すので、スクロールする欄の中でも切れない。
// もう一度押す・ほかの所を押す・Escキー・スクロールで閉じる。
// 見た目は各画面のCSSの .help / .help-pop。
// broadcaster-app-tauri/viewer-app-tauri の shared/help.js は同じ内容。
(function () {
  let pop = null;
  let owner = null;

  function close() {
    if (pop) pop.remove();
    pop = null;
    owner = null;
  }

  function open(el) {
    close();
    owner = el;
    pop = document.createElement('div');
    pop.className = 'help-pop';
    pop.textContent = el.dataset.help;
    document.body.appendChild(pop);
    const r = el.getBoundingClientRect();
    const w = pop.offsetWidth;
    const h = pop.offsetHeight;
    const left = Math.max(8, Math.min(r.left - 12, window.innerWidth - w - 8));
    // 下に入りきらない時は上に出す
    const below = r.bottom + 6;
    const top = below + h > window.innerHeight - 8 ? Math.max(8, r.top - h - 6) : below;
    pop.style.left = `${left}px`;
    pop.style.top = `${top}px`;
  }

  function setup(root) {
    for (const el of (root || document).querySelectorAll('.help[data-help]:not([data-help-ready])')) {
      el.dataset.helpReady = '1';
      el.textContent = 'ⓘ';
      el.setAttribute('role', 'button');
      el.setAttribute('tabindex', '0');
      el.setAttribute('aria-label', '説明を見る');
    }
  }

  document.addEventListener('click', (e) => {
    const el = e.target.closest && e.target.closest('.help[data-help]');
    if (el) {
      // ラベルの中のⓘを押した時に、チェックボックスが切り替わらないように
      e.preventDefault();
      e.stopPropagation();
      if (owner === el) close();
      else open(el);
      return;
    }
    if (pop && !pop.contains(e.target)) close();
  }, true);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') close();
    const el = e.target.closest && e.target.closest('.help[data-help]');
    if (el && (e.key === 'Enter' || e.key === ' ')) {
      e.preventDefault();
      if (owner === el) close();
      else open(el);
    }
  });
  document.addEventListener('scroll', close, true);
  window.addEventListener('resize', close);

  // 後から作られる欄(JSで作る一覧など)の分は、作った後にHelpTips.setup(要素)を呼ぶ
  window.HelpTips = { setup, close };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', () => setup());
  else setup();
})();
