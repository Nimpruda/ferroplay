// Example JS plugin: a speed button in the control bar.
// It reads the DOM, but acts only by sending intents to the Rust core.
const RATES = [1, 1.25, 1.5, 2, 0.5, 0.75];

export function speed({ root, video, send }) {
  const button = document.createElement('button');
  button.setAttribute('aria-label', 'Playback speed');
  button.style.cssText = 'width: auto; min-width: 3em; font-variant-numeric: tabular-nums';
  root.querySelector('[data-do=toggleFullscreen]').before(button);

  const show = () => { button.textContent = `${video.playbackRate}×`; };
  button.addEventListener('click', () => {
    const next = RATES[(RATES.indexOf(video.playbackRate) + 1) % RATES.length];
    send('rate', next);
  });
  video.addEventListener('ratechange', show);
  show();

  return () => video.removeEventListener('ratechange', show);
}
