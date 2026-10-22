// End-to-end check of the website's download sections in headless Chromium.
//
// Serves `site` and opens each app's page, Social Deduction's and Twenty
// Questions', as visitors on each platform. On macOS and Windows, told by
// navigator.userAgentData or by the user agent alone, the button says the
// platform and downloads its installer, with that platform's one extra
// step under it. On Linux it goes to the list's Linux rows, which it marks.
// Phones, tablets and Chromebooks get no button, and neither does a visitor
// without JavaScript. The list of every platform is there in each case,
// with the six installers, and the page offers no curl | sh, .msi or
// AppImage. Then the site is built as the website's build does, with a
// release of Social Deduction and none of Twenty Questions: Social
// Deduction's downloads go to its release, and Twenty Questions' page says
// its first release is on its way, with no button and no list. The
// console stays clean throughout.
//
//   npm run test:site              # against ../../../../site, from this directory
//   node site.mjs <site>           # against another copy

import { execFileSync } from 'node:child_process';
import { cpSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';
import { serve } from '../../../../crates/gui/e2e/serve.mjs';

const site = resolve(process.argv[2] ?? '../../../../site');

let failures = 0;
function check(condition, label) {
  console.log(`${condition ? 'ok  ' : 'FAIL'} ${label}`);
  if (!condition) failures += 1;
}

/** Each app's page and its installers, each linked through the placeholder
 * that the website's build resolves to the app's newest release. */
const apps = [
  { name: 'Social Deduction', page: 'social-deduction.html', product: 'social-deduction', stem: 'Social-Deduction' },
  { name: 'Twenty Questions', page: 'twenty-questions.html', product: 'twenty-questions', stem: 'Twenty-Questions' },
].map((app) => {
  const download = `{{release:${app.product}}}/`;
  return {
    ...app,
    installers: [
      `${app.stem}.dmg`,
      `${app.stem}-setup.exe`,
      `${app.product}-amd64.deb`,
      `${app.product}-x86_64.rpm`,
      `${app.product}-arm64.deb`,
      `${app.product}-aarch64.rpm`,
    ].map((name) => download + name),
    // What each platform's visitor is offered: the button's label and target.
    offered: {
      macos: { label: 'Download for macOS', href: `${download}${app.stem}.dmg` },
      windows: { label: 'Download for Windows', href: `${download}${app.stem}-setup.exe` },
      linux: { label: 'Download for Linux', href: '#linux' },
    },
  };
});

const agents = {
  macos: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15',
  windows: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:131.0) Gecko/20100101 Firefox/131.0',
  linux: 'Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0',
  android: 'Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36',
  iphone: 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1',
  chromebook: 'Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36',
};

// How navigator.userAgentData names each platform.
const platformNames = { macos: 'macOS', windows: 'Windows', linux: 'Linux' };

const browser = await chromium.launch();

// Opens the site as a visitor with the given user agent and, when given,
// navigator.userAgentData's platform; `null` takes userAgentData away, as
// Firefox and Safari have none. `touch` is navigator.maxTouchPoints, and
// `after` acts on the page before it is read. `origin` is the page's address.
async function visit(origin, { userAgent, platform = null, touch = 0, javaScript = true, after = async () => {} }) {
  const context = await browser.newContext({ userAgent, javaScriptEnabled: javaScript });
  await context.addInitScript(({ platform, touch }) => {
    Object.defineProperty(Navigator.prototype, 'userAgentData', {
      get: () => (platform === null ? undefined : { platform, mobile: false, brands: [] }),
    });
    Object.defineProperty(Navigator.prototype, 'maxTouchPoints', { get: () => touch });
  }, { platform, touch });
  const page = await context.newPage();
  const errors = [];
  page.on('console', (message) => {
    if (message.type() === 'error' || message.type() === 'warning') errors.push(message.text());
  });
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(origin);
  await after(page);
  const seen = await page.evaluate(() => {
    const button = document.querySelector('#download .button');
    const shown = (element) => element !== null && element.checkVisibility();
    return {
      button: shown(button) ? { label: button.textContent.trim(), href: button.getAttribute('href') } : null,
      steps: [...document.querySelectorAll('#download [data-platform]')].filter(shown).map((p) => p.dataset.platform),
      links: [...document.querySelectorAll('.downloads a')].filter(shown).map((a) => a.getAttribute('href')),
      marked: [...document.querySelectorAll('.downloads li')]
        .filter((li) => getComputedStyle(li).backgroundColor !== 'rgba(0, 0, 0, 0)')
        .map((li) => li.querySelector('.platform')?.textContent.trim()),
      text: document.body.innerText,
      list: document.querySelector('.downloads') !== null,
      hrefs: [...document.querySelectorAll('a')].map((a) => a.href),
    };
  });
  await context.close();
  return { ...seen, errors };
}

function everything(app, what, seen) {
  check(JSON.stringify(seen.links) === JSON.stringify(app.installers), `${what}: the list has every installer`);
  check(!/curl|installer\.(sh|ps1)|irm /.test(seen.text) && !seen.hrefs.some((h) => /installer\.(sh|ps1)$/.test(h)),
    `${what}: no curl | sh`);
  check(!seen.hrefs.some((h) => /\.(msi|AppImage)$/i.test(h)), `${what}: no .msi or AppImage`);
  check(seen.errors.length === 0, `${what}: the console stays clean ${seen.errors.join(' | ')}`);
}

function offers(app, what, seen, platform) {
  const { label, href } = app.offered[platform];
  check(seen.button?.label === label, `${what}: the button says ${label}`);
  check(seen.button?.href === href, `${what}: the button goes to ${href}`);
  check(JSON.stringify(seen.steps) === JSON.stringify([platform]), `${what}: the ${platform} step alone`);
  everything(app, what, seen);
}

function none(app, what, seen) {
  check(seen.button === null, `${what}: no button`);
  check(seen.steps.length === 0, `${what}: no step`);
  everything(app, what, seen);
}

const { server, origin: root } = await serve(site);
for (const app of apps) {
  const origin = new URL(app.page, root).href;
  const at = (options) => visit(origin, options);
  const named = (what) => `${app.name}, ${what}`;
  for (const platform of Object.keys(app.offered)) {
    // userAgentData where the browser has it, as Chromium does, which wins
    // over the user agent.
    const other = platform === 'macos' ? agents.windows : agents.macos;
    offers(app, named(`${platform} by userAgentData`), await at({ userAgent: other, platform: platformNames[platform] }), platform);
    // The user agent alone, as Firefox and Safari tell it.
    offers(app, named(`${platform} by user agent`), await at({ userAgent: agents[platform] }), platform);
  }

  // Platforms with no installer.
  none(app, named('Android by userAgentData'), await at({ userAgent: agents.android, platform: 'Android' }));
  none(app, named('Android by user agent'), await at({ userAgent: agents.android }));
  none(app, named('iPhone'), await at({ userAgent: agents.iphone }));
  // An iPad's Safari says it is a Mac, but a Mac has no touch screen.
  none(app, named('iPad'), await at({ userAgent: agents.macos, touch: 5 }));
  none(app, named('Chromebook by userAgentData'), await at({ userAgent: agents.chromebook, platform: 'Chrome OS' }));
  none(app, named('Chromebook by user agent'), await at({ userAgent: agents.chromebook }));
  none(app, named('an unknown platform'), await at({ userAgent: 'Mozilla/5.0 (Plan 9)' }));
  none(app, named('without JavaScript'), await at({ userAgent: agents.macos, javaScript: false }));

  // The Linux button lands on the Linux rows, which stand out, and only those.
  const linux = await at({ userAgent: agents.linux, after: (page) => page.click('#download .button') });
  check(JSON.stringify(linux.marked) === JSON.stringify(['Linux x86_64', 'Linux arm64']),
    named(`the Linux button marks the Linux rows ${linux.marked.join(' ')}`));
  everything(app, named('after the Linux button'), linux);
  const windows = await at({ userAgent: agents.windows, after: (page) => page.goto(`${origin}#windows`) });
  check(windows.marked.length === 0, named(`another platform's row marks nothing ${windows.marked.join(' ')}`));
}
server.close();

// The site as the website's build leaves it, with a release of Social
// Deduction and none yet of Twenty Questions.
const built = mkdtempSync(join(tmpdir(), 'site-'));
cpSync(site, built, { recursive: true });
const releases = [{ tag_name: 'social-deduction-v0.2.0', draft: false, prerelease: false }];
execFileSync(resolve('../../../../.github/scripts/site-releases.sh'), [built], { input: JSON.stringify(releases), stdio: ['pipe', 'ignore', 'ignore'] });
const deployed = await serve(built);
const release = 'https://github.com/wpm/FreeAgent/releases/download/social-deduction-v0.2.0/';
const [released, coming] = apps;
const resolved = await visit(new URL(released.page, deployed.origin).href, { userAgent: agents.macos });
check(resolved.button?.href === `${release}${released.stem}.dmg`, `${released.name} released: the button downloads from the release (${resolved.button?.href})`);
check(JSON.stringify(resolved.links) === JSON.stringify(released.installers.map((link) => link.replace(`{{release:${released.product}}}/`, release))),
  `${released.name} released: the list links the release's installers`);
check(!resolved.text.includes('on its way'), `${released.name} released: nothing says a release is on its way`);
for (const [what, options] of [
  ['macOS', { userAgent: agents.macos }],
  ['Linux', { userAgent: agents.linux }],
  ['without JavaScript', { userAgent: agents.macos, javaScript: false }],
]) {
  const seen = await visit(new URL(coming.page, deployed.origin).href, options);
  const label = `${coming.name} before its first release, ${what}`;
  check(seen.text.includes('The first release is on its way.'), `${label}: the first release is on its way`);
  check(seen.button === null && !seen.list && seen.links.length === 0, `${label}: no button and no downloads`);
  check(!seen.hrefs.some((href) => /releases\/download|\.(dmg|exe|deb|rpm|msi|AppImage)$/.test(href)), `${label}: no link to a release`);
  check(seen.errors.length === 0, `${label}: the console stays clean ${seen.errors.join(' | ')}`);
}
deployed.server.close();
rmSync(built, { recursive: true, force: true });

await browser.close();
process.exit(failures === 0 ? 0 : 1);
