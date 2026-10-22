// The download button, for the visitor's platform. navigator.userAgentData
// names the platform where the browser has it, and the user agent where it
// doesn't. The button takes its target from the list of every platform
// under it, and stays hidden on a platform with no installer, so without
// JavaScript or a platform the list is all there is. Before a product's
// first release its page has neither.
{
  // Phones, tablets and Chromebooks run none of the installers. An iPad's
  // Safari says it is a Mac, but a Mac has no touch screen.
  const detect = () => {
    const name = navigator.userAgentData?.platform || navigator.userAgent;
    if (/android|iphone|ipad|ipod|cros|chrome os/i.test(name)) return null;
    if (/mac/i.test(name)) return navigator.maxTouchPoints > 1 ? null : 'macos';
    if (/win/i.test(name)) return 'windows';
    if (/linux|x11/i.test(name)) return 'linux';
    return null;
  };

  const labels = { macos: 'macOS', windows: 'Windows', linux: 'Linux' };
  const platform = detect();
  const download = document.getElementById('download');
  if (platform && download) {
    const button = download.querySelector('.button');
    button.textContent = `Download for ${labels[platform]}`;
    // Browsers on Linux don't say the architecture reliably, so the Linux
    // button goes to the list's Linux rows rather than to a file.
    button.href = platform === 'linux'
      ? '#linux'
      : document.querySelector(`#${platform} a`).getAttribute('href');
    download.querySelector(`[data-platform="${platform}"]`).hidden = false;
    download.hidden = false;
  }
}
