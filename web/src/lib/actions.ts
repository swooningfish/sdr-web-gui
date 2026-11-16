/** `use:indeterminate={bool}`: the "some selected" state of a select-all checkbox. */
export function indeterminate(node: HTMLInputElement, on: boolean) {
  node.indeterminate = on;
  return {
    update(v: boolean) {
      node.indeterminate = v;
    },
  };
}

const COLLAPSED_KEY = 'sdr.panels.collapsed';

function readCollapsed(): Record<string, boolean> {
  try {
    const v = JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? '{}');
    return v && typeof v === 'object' ? v : {};
  } catch {
    return {};
  }
}

/**
 * `use:collapsible={'id'}` on a `.panel`: its `<h2>` (direct, or inside a `.head`) becomes a
 * show/hide toggle, and everything but the heading row is hidden while closed. The open or
 * closed state is remembered per id in this browser.
 */
export function collapsible(node: HTMLElement, id: string) {
  const h2 = node.querySelector<HTMLHeadingElement>(':scope > h2, :scope > .head > h2');
  if (!h2) return;
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'collapse-toggle';
  while (h2.firstChild) btn.append(h2.firstChild);
  h2.append(btn);

  const apply = (closed: boolean) => {
    if (closed) node.dataset.collapsed = '';
    else delete node.dataset.collapsed;
    btn.setAttribute('aria-expanded', String(!closed));
  };
  apply(readCollapsed()[id] === true);

  btn.addEventListener('click', () => {
    const closed = !('collapsed' in node.dataset);
    apply(closed);
    const all = readCollapsed();
    if (closed) all[id] = true;
    else delete all[id];
    try {
      localStorage.setItem(COLLAPSED_KEY, JSON.stringify(all));
    } catch {
      /* private mode: just don't remember */
    }
  });
}
