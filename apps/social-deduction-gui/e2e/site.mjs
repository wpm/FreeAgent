// End-to-end check of the website's download section in headless Chromium.
//
// Serves `site` and opens its Social Deduction page as visitors on each
// platform. On macOS and Windows, told by navigator.userAgentData or by the
// user agent alone, the button says the platform and downloads its
// installer, with that platform's one extra step under it. On Linux it goes
// to the list's Linux rows, which it marks. Phones, tablets and Chromebooks get no button, and
// neither does a visitor without JavaScript. The list of every platform is
// there in each case, with the six installers, and the page offers no
// curl | sh, .msi or AppImage. The console stays clean throughout.
//
//   npm run test:site              # against ../../../site, from this directory
//   node site.mjs <site>           # against another copy

import { resolve } from 'node:path';
import { chromium } from 'playwright';
import { serve } from './serve.mjs';

const { server, origin: root } = await serve(resolve(process.argv[2] ?? '../../../site'));
const origin = new URL('social-deduction.html', root).href;

let failures = 0;
function check(condition, label) {
  console.log(`${condition ? 'ok  ' : 'FAIL'} ${label}`);
  if (!condition) failures += 1;
}

// The website's build resolves the placeholder to the newest release.
const download = '{{release:social-deduction}}/';
const installers = [
  'Social-Deduction.dmg',
  'Social-Deduction-setup.exe',
  'social-deduction-amd64.deb',
  'social-deduction-x86_64.rpm',
  'social-deduction-arm64.deb',
  'social-deduction-aarch64.rpm',
].map((name) => download + name);

const agents = {
  macos: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15',
  windows: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:131.0) Gecko/20100101 Firefox/131.0',
  linux: 'Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0',
  android: 'Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36',
  iphone: 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1',
  chromebook: 'Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36',
};

// What each platform's visitor is offered: the button's label and target,
// and the step shown under it.
const offered = {
  macos: { label: 'Download for macOS', href: download + 'Social-Deduction.dmg' },
  windows: { label: 'Download for Windows', href: download + 'Social-Deduction-setup.exe' },
  linux: { label: 'Download for Linux', href: '#linux' },
};
// How navigator.userAgentData names each platform.
const platformNames = { macos: 'macOS', windows: 'Windows', linux: 'Linux' };

const browser = await chromium.launch();

// Opens the site as a visitor with the given user agent and, when given,
// navigator.userAgentData's platform; `null` takes userAgentData away, as
// Firefox and Safari have none. `touch` is navigator.maxTouchPoints, and
// `after` acts on the page before it is read.
async function visit({ userAgent, platform = null, touch = 0, javaScript = true, after = async () => {} }) {
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
      hrefs: [...document.querySelectorAll('a')].map((a) => a.href),
    };
  });
  await context.close();
  return { ...seen, errors };
}

function everything(what, seen) {
  check(JSON.stringify(seen.links) === JSON.stringify(installers), `${what}: the list has every installer`);
  check(!/curl|installer\.(sh|ps1)|irm /.test(seen.text) && !seen.hrefs.some((h) => /installer\.(sh|ps1)$/.test(h)),
    `${what}: no curl | sh`);
  check(!seen.hrefs.some((h) => /\.(msi|AppImage)$/i.test(h)), `${what}: no .msi or AppImage`);
  check(seen.errors.length === 0, `${what}: the console stays clean ${seen.errors.join(' | ')}`);
}

function offers(what, seen, platform) {
  const { label, href } = offered[platform];
  check(seen.button?.label === label, `${what}: the button says ${label}`);
  check(seen.button?.href === href, `${what}: the button goes to ${href}`);
  check(JSON.stringify(seen.steps) === JSON.stringify([platform]), `${what}: the ${platform} step alone`);
  everything(what, seen);
}

function none(what, seen) {
  check(seen.button === null, `${what}: no button`);
  check(seen.steps.length === 0, `${what}: no step`);
  everything(what, seen);
}

for (const platform of Object.keys(offered)) {
  // userAgentData where the browser has it, as Chromium does, which wins
  // over the user agent.
  const other = platform === 'macos' ? agents.windows : agents.macos;
  offers(`${platform} by userAgentData`, await visit({ userAgent: other, platform: platformNames[platform] }), platform);
  // The user agent alone, as Firefox and Safari tell it.
  offers(`${platform} by user agent`, await visit({ userAgent: agents[platform] }), platform);
}

// Platforms with no installer.
none('Android by userAgentData', await visit({ userAgent: agents.android, platform: 'Android' }));
none('Android by user agent', await visit({ userAgent: agents.android }));
none('iPhone', await visit({ userAgent: agents.iphone }));
// An iPad's Safari says it is a Mac, but a Mac has no touch screen.
none('iPad', await visit({ userAgent: agents.macos, touch: 5 }));
none('Chromebook by userAgentData', await visit({ userAgent: agents.chromebook, platform: 'Chrome OS' }));
none('Chromebook by user agent', await visit({ userAgent: agents.chromebook }));
none('an unknown platform', await visit({ userAgent: 'Mozilla/5.0 (Plan 9)' }));
none('without JavaScript', await visit({ userAgent: agents.macos, javaScript: false }));

// The Linux button lands on the Linux rows, which stand out, and only those.
const linux = await visit({ userAgent: agents.linux, after: (page) => page.click('#download .button') });
check(JSON.stringify(linux.marked) === JSON.stringify(['Linux x86_64', 'Linux arm64']),
  `the Linux button marks the Linux rows ${linux.marked.join(' ')}`);
everything('after the Linux button', linux);
const windows = await visit({ userAgent: agents.windows, after: (page) => page.goto(`${origin}#windows`) });
check(windows.marked.length === 0, `another platform's row marks nothing ${windows.marked.join(' ')}`);

await browser.close();
server.close();
process.exit(failures === 0 ? 0 : 1);
