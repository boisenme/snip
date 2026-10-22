// Renders the demo scenes to GIFs, frame by frame (deterministic: each frame is drawn at an
// exact time t, nothing is recorded in real time).
//
//   ./render.sh                 # both scenes -> ../hero.gif, ../ocr.gif
//   ./render.sh hero            # one scene
//
// Needs: google-chrome (or CHROME=/path/to/chrome), ffmpeg, node, and puppeteer-core
// (render.sh installs it into a cache dir; it is never added to the project).
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const req = createRequire(process.env.PUPPETEER_DIR ? join(process.env.PUPPETEER_DIR, 'package.json') : import.meta.url);
const puppeteer = (await import(pathToFileURL(req.resolve('puppeteer-core')))).default;

const SCENES = {
  hero: { fps: 25, width: 960 },
  ocr: { fps: 25, width: 960 },
};
const DPR = 2; // render at 2x, downscale with lanczos for crisp text
const names = process.argv.slice(2).length ? process.argv.slice(2) : Object.keys(SCENES);

const browser = await puppeteer.launch({
  executablePath: process.env.CHROME || '/usr/bin/google-chrome-stable',
  headless: true,
  args: ['--hide-scrollbars', '--font-render-hinting=none', '--force-color-profile=srgb'],
});
for (const name of names) {
  const { fps, width } = SCENES[name];
  const page = await browser.newPage();
  await page.setViewport({ width: 1120, height: 630, deviceScaleFactor: DPR });
  await page.goto(pathToFileURL(join(here, `${name}.html`)).href + `?capture&dpr=${DPR}`);
  await page.waitForFunction('window.sceneReady === true');
  const duration = await page.evaluate('window.sceneDuration');
  const stage = await page.$('#stage');
  const frames = mkdtempSync(join(tmpdir(), `snip-${name}-`));
  const n = Math.round(duration * fps);
  for (let i = 0; i < n; i++) {
    await page.evaluate((t) => window.renderAt(t), i / fps);
    await stage.screenshot({ path: join(frames, `f${String(i).padStart(4, '0')}.png`) });
  }
  await page.close();
  const out = join(here, '..', `${name}.gif`);
  const filter = `scale=${width}:-1:flags=lanczos,split[a][b];` +
    `[a]palettegen=max_colors=256:stats_mode=full[p];[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle`;
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-framerate', String(fps), '-i', join(frames, 'f%04d.png'),
    '-filter_complex', filter, '-loop', '0', out], { stdio: 'inherit' });
  if (!process.env.KEEP_FRAMES) rmSync(frames, { recursive: true });
  else console.log(`frames kept in ${frames}`);
  console.log(`${out}: ${n} frames, ${(statSync(out).size / 1e6).toFixed(2)} MB`);
}
await browser.close();
