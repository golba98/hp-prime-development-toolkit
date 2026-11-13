import { test, expect } from '@playwright/test';
import { spawn } from 'node:child_process';
import path from 'node:path';
const binary =
  process.env.HPDEV_BINARY ||
  path.resolve(
    'target/debug',
    process.platform === 'win32' ? 'hpdev.exe' : 'hpdev',
  );
async function server(file) {
  const child = spawn(binary, [
    'preview',
    file,
    '--web',
    '--no-open',
    '--json',
    '--max-time',
    '10',
  ]);
  let stdout = '',
    stderr = '';
  child.stdout.on('data', (b) => (stdout += b));
  const url = await new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`No browser URL: ${stderr}`)),
      5000,
    );
    child.stderr.on('data', (b) => {
      stderr += b;
      const match = stderr.match(/Browser preview: (http:\/\/[^\s]+)/);
      if (match) {
        clearTimeout(timer);
        resolve(match[1]);
      }
    });
    child.on('error', reject);
  });
  return {
    url,
    async stop() {
      if (child.exitCode === null) {
        child.kill('SIGINT');
        await new Promise((resolve) => {
          const timer = setTimeout(() => {
            child.kill('SIGKILL');
            resolve();
          }, 3000);
          child.on('close', () => {
            clearTimeout(timer);
            resolve();
          });
        });
      }
      return JSON.parse(stdout);
    },
  };
}
test('canvas receives all five actual frames', async ({ page }) => {
  const app = await server('examples/LIVE.hpprgm');
  try {
    await page.addInitScript(() => {
      window.seenColors = [];
      const original = CanvasRenderingContext2D.prototype.putImageData;
      CanvasRenderingContext2D.prototype.putImageData = function (
        image,
        ...args
      ) {
        window.seenColors.push(Array.from(image.data.slice(0, 3)));
        return original.call(this, image, ...args);
      };
    });
    await page.goto(app.url);
    await expect(page.locator('#status')).toHaveText('success');
    const colors = await page.evaluate(() => window.seenColors);
    for (const red of [40, 80, 120, 160, 200])
      expect(colors).toContainEqual([red, 0, 0]);
    expect(
      await page
        .locator('canvas')
        .evaluate((canvas) =>
          Array.from(canvas.getContext('2d').getImageData(0, 0, 1, 1).data),
        ),
    ).toEqual([200, 0, 0, 255]);
    await page.screenshot({
      path: 'test-results/live-browser.png',
      fullPage: true,
    });
  } finally {
    const result = await app.stop();
    expect(result.status).toBe('success');
    expect(result.return_value.value).toBe('5');
  }
});
test('browser INPUT, CHOOSE and message controls write back values', async ({
  page,
}) => {
  const app = await server('examples/INPUT.ppl');
  try {
    await page.goto(app.url);
    await expect(page.locator('dialog')).toBeVisible();
    await page.getByLabel('Value').fill('7');
    await page.getByRole('button', { name: 'OK', exact: true }).click();
    await expect(page.locator('#title')).toHaveText('Add to number');
    await page.locator('select').selectOption('2');
    await page.getByRole('button', { name: 'OK', exact: true }).click();
    await expect(page.locator('#message')).toHaveText('Result: 9');
    await page.getByRole('button', { name: 'OK', exact: true }).click();
    await expect(page.locator('#status')).toHaveText('success');
    await expect(page.locator('#output')).toHaveText('9');
  } finally {
    const result = await app.stop();
    expect(result.return_value.value).toBe('9');
  }
});
test('Stop terminates a native WAIT', async ({ page }) => {
  const app = await server('tests/fixtures/WAIT.ppl');
  try {
    await page.goto(app.url);
    await expect(page.locator('canvas')).toHaveAttribute(
      'data-sequence',
      /\d+/,
    );
    await page.getByRole('button', { name: 'Stop program' }).click();
    await expect(page.locator('#status')).toContainText('cancelled');
  } finally {
    expect((await app.stop()).status).toBe('cancelled');
  }
});
test('browser displays compiler codes and source locations', async ({
  page,
}) => {
  const app = await server('tests/fixtures/INVALID.ppl');
  try {
    await page.goto(app.url);
    await expect(page.locator('#status')).toContainText('invalid_source');
    await expect(page.locator('#output')).toContainText(
      'undeclared-identifier',
    );
    await expect(page.locator('#output')).toContainText(':3:10');
  } finally {
    expect((await app.stop()).status).toBe('invalid_source');
  }
});
