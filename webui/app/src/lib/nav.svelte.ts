// 自写 hash 路由：#/library #/steward #/console #/settings（无路由库，四视图）

export type View = 'library' | 'steward' | 'console' | 'settings';

const VIEWS: View[] = ['library', 'steward', 'console', 'settings'];

function parseHash(): View {
  const h = location.hash.replace(/^#\/?/, '');
  return (VIEWS as string[]).includes(h) ? (h as View) : 'library';
}

class Nav {
  view = $state<View>(parseHash());

  constructor() {
    window.addEventListener('hashchange', () => {
      this.view = parseHash();
    });
  }

  go(v: View) {
    location.hash = '/' + v;
    this.view = v;
  }
}

export const nav = new Nav();
