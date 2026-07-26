// 自写 hash 路由：#/library #/steward #/console #/settings #/item/{id} #/player/{item}/{file}
// 支持 query（#/library?genre=X、#/console?task=3）

export type View = 'library' | 'steward' | 'console' | 'settings' | 'item' | 'player';

const VIEWS: View[] = ['library', 'steward', 'console', 'settings'];

interface Route {
  view: View;
  itemId: number | null;
  fileId: number | null;
  query: Record<string, string>;
}

function parseHash(): Route {
  const raw = location.hash.replace(/^#\/?/, '');
  const [pathPart, queryPart] = raw.split('?', 2);
  const query: Record<string, string> = {};
  if (queryPart) {
    for (const [k, v] of new URLSearchParams(queryPart)) query[k] = v;
  }
  const segs = pathPart.split('/').filter(Boolean);
  if (segs[0] === 'item' && segs[1] && /^\d+$/.test(segs[1])) {
    return { view: 'item', itemId: Number(segs[1]), fileId: null, query };
  }
  if (
    segs[0] === 'player' &&
    segs[1] && /^\d+$/.test(segs[1]) &&
    segs[2] && /^\d+$/.test(segs[2])
  ) {
    return { view: 'player', itemId: Number(segs[1]), fileId: Number(segs[2]), query };
  }
  const v = (VIEWS as string[]).includes(segs[0]) ? (segs[0] as View) : 'library';
  return { view: v, itemId: null, fileId: null, query };
}

class Nav {
  #route = $state<Route>(parseHash());

  constructor() {
    window.addEventListener('hashchange', () => {
      this.#route = parseHash();
    });
  }

  get view(): View {
    return this.#route.view;
  }
  get itemId(): number | null {
    return this.#route.itemId;
  }
  get fileId(): number | null {
    return this.#route.fileId;
  }
  get query(): Record<string, string> {
    return this.#route.query;
  }

  go(v: 'library' | 'steward' | 'console' | 'settings', query?: Record<string, string>) {
    location.hash = '/' + v + this.#qs(query);
  }

  goItem(id: number) {
    location.hash = `/item/${id}`;
  }

  goPlayer(itemId: number, fileId: number) {
    location.hash = `/player/${itemId}/${fileId}`;
  }

  #qs(query?: Record<string, string>): string {
    if (!query) return '';
    const q = new URLSearchParams();
    for (const [k, v] of Object.entries(query)) if (v) q.set(k, v);
    const s = q.toString();
    return s ? '?' + s : '';
  }
}

export const nav = new Nav();
