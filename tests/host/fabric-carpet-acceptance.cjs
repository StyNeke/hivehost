#!/usr/bin/env node
// Browser acceptance: sign in, import and qualify the Fabric egg, add an address, create a Fabric
// server, install Carpet from Modrinth, start it, spawn Carpet bots from the console and confirm
// they joined, then stop gracefully. Drives only the real interface over HTTPS.
//
// Required environment: GM_URL (https://...), GM_USER, GM_PASSWORD, GM_EGG (path to the Fabric
// egg JSON), GM_ADDRESS_IP, GM_ADDRESS_PORT. Optional: GM_OUT (screenshot directory),
// GM_IGNORE_TLS=1 for a self-signed test certificate. Requires Playwright with Chromium.
const { chromium } = require('playwright');
const path = require('path');

const env = name => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
};
const url = env('GM_URL');
const out = process.env.GM_OUT || '.';
const bots = ['Bot_One', 'Bot_Two', 'Bot_Three'];
const started = Date.now();
const log = message => console.log(`[${((Date.now() - started) / 1000).toFixed(0).padStart(4)}s] ${message}`);

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ ignoreHTTPSErrors: process.env.GM_IGNORE_TLS === '1', viewport: { width: 1280, height: 900 } });
  const shot = name => page.screenshot({ path: path.join(out, `${name}.png`), fullPage: true });
  const pill = () => page.locator('.status-pill');
  try {
    await page.goto(url);
    await page.getByLabel('Username').fill(env('GM_USER'));
    await page.getByLabel('Password').fill(env('GM_PASSWORD'));
    await page.getByRole('button', { name: 'Sign in' }).click();
    await page.getByRole('heading', { name: 'Servers' }).waitFor();
    await page.waitForLoadState('networkidle');
    log('signed in');

    // Repeatable runs: remove a server left by an earlier run through the interface.
    const leftover = page.locator('.server-card', { hasText: 'Carpet Bots' });
    if (await leftover.count()) {
      await leftover.first().click();
      await page.getByRole('tab', { name: /Delete/ }).click();
      await page.getByLabel(/to confirm/).fill('Carpet Bots');
      await page.getByRole('button', { name: 'Delete server' }).click();
      await page.getByText('Carpet Bots was deleted.').waitFor({ timeout: 3 * 60 * 1000 });
      log('deleted the server left by an earlier run');
    }

    await page.getByRole('button', { name: 'Egg library', exact: true }).click();
    if (!(await page.locator('.egg-card', { hasText: 'Fabric' }).count())) {
      await page.getByLabel('Egg JSON file').setInputFiles(env('GM_EGG'));
      await page.getByRole('button', { name: 'Import egg' }).click();
      await page.getByText('Egg imported into the shared library.').waitFor();
      log('imported the Fabric egg');
    }
    await page.locator('.egg-card', { hasText: 'Fabric' }).click();
    await page.locator('.egg-details h2', { hasText: 'Fabric' }).waitFor();
    const qualify = page.getByRole('button', { name: 'Mark runtime qualified' });
    if (await qualify.isVisible().catch(() => false)) {
      await qualify.click();
      await page.getByText('Teammates can now create servers from this egg revision.').waitFor();
      log('qualified the Fabric revision');
    }

    await page.getByRole('button', { name: 'Servers', exact: true }).click();
    await page.locator('.page-heading').getByRole('button', { name: 'Add server' }).click();
    const panel = page.getByRole('region', { name: 'New server' });
    await panel.waitFor();
    const address = `${env('GM_ADDRESS_IP')}:${env('GM_ADDRESS_PORT')}`;
    if (!(await panel.locator('#server-allocation option', { hasText: address }).count())) {
      await panel.getByLabel('IP').fill(env('GM_ADDRESS_IP'));
      await panel.getByLabel('Port').fill(env('GM_ADDRESS_PORT'));
      await panel.getByRole('button', { name: 'Add address' }).click();
      await panel.locator('#server-allocation option', { hasText: address }).waitFor({ state: 'attached' });
      log(`added address ${address}`);
    }
    await panel.getByLabel('Server name').fill('Carpet Bots');
    const game = panel.getByLabel('Game', { exact: true });
    const fabric = await game.locator('option', { hasText: /^Fabric/ }).first().getAttribute('value');
    await game.selectOption(fabric);
    await panel.getByLabel('Environment').waitFor();
    await panel.getByLabel('Address').selectOption({ label: `${address} (TCP)` });
    await panel.getByLabel('RAM (GiB)').fill('3');
    await panel.getByLabel('Disk (GiB)').fill('4');
    await panel.getByLabel('CPU (cores)').fill('2');
    await panel.getByLabel(/accept the game's license/).check();
    await shot('01-create');
    await panel.getByRole('button', { name: 'Create server' }).click();
    await page.getByRole('heading', { name: 'Carpet Bots' }).waitFor();
    log('created the server; installing');

    await pill().filter({ hasText: /^(Offline|Install failed)$/ }).waitFor({ timeout: 15 * 60 * 1000 });
    if ((await pill().textContent()) === 'Install failed') throw new Error(await page.locator('.install-error').textContent());
    log(`installed: ${await page.locator('.page-heading p').textContent()}`);
    await shot('02-installed');

    await page.getByRole('tab', { name: /Mods/ }).click();
    await page.getByLabel('Search Modrinth').fill('carpet');
    await page.getByRole('button', { name: 'Search', exact: true }).click();
    const carpet = page.locator('.mod-list li', { has: page.locator('strong', { hasText: /^Carpet$/ }) }).first();
    await carpet.getByRole('button', { name: 'Install' }).click();
    await page.getByText(/Installing Carpet/).waitFor();
    log(`requested: ${await page.locator('.feedback.success.inline').textContent()}`);
    for (let attempt = 0; ; attempt++) {
      await page.waitForTimeout(2000);
      await page.getByRole('tab', { name: /Activity/ }).click();
      await page.getByRole('tab', { name: /Mods/ }).click();
      if (await page.getByRole('button', { name: 'Remove Carpet' }).isVisible().catch(() => false)) break;
      if (attempt > 60) throw new Error('Carpet did not appear as installed');
    }
    log(`installed mod: ${(await page.locator('.mod-list li', { hasText: 'Carpet' }).last().textContent()).trim()}`);
    await shot('03-mods');

    await page.getByRole('tab', { name: /Console/ }).click();
    await page.getByRole('button', { name: 'Start', exact: true }).click();
    await pill().filter({ hasText: /^Running$/ }).waitFor({ timeout: 5 * 60 * 1000 });
    log('server is running (readiness marker seen)');
    const consoleInput = page.getByLabel('Console command');
    for (const bot of bots) {
      await consoleInput.fill(`player ${bot} spawn`);
      await consoleInput.press('Enter');
      await page.waitForFunction(() => document.querySelector('input[aria-label="Console command"]').value === '');
      log(`sent: player ${bot} spawn`);
    }
    const output = page.getByRole('log', { name: 'Console output' });
    for (let attempt = 0; ; attempt++) {
      await consoleInput.fill('list');
      await consoleInput.press('Enter');
      await page.waitForTimeout(3000);
      const text = await output.textContent();
      if (text.includes(`There are ${bots.length} of a max`)) break;
      if (attempt > 40) throw new Error(`bots did not join:\n${text.slice(-2000)}`);
    }
    const lines = (await output.innerText()).split('\n');
    for (const line of lines.filter(l => /Bot_|players online/.test(l)).slice(-8)) log(`console: ${line}`);
    await shot('04-bots');

    await page.getByRole('button', { name: 'Stop', exact: true }).click();
    await pill().filter({ hasText: /^Offline$/ }).waitFor({ timeout: 3 * 60 * 1000 });
    log('stopped gracefully');
    await page.getByRole('tab', { name: /Activity/ }).click();
    await page.locator('.activity-list li').first().waitFor();
    await shot('05-activity');
    log('acceptance passed');
  } catch (error) {
    await shot('failure').catch(() => {});
    console.error(error);
    process.exitCode = 1;
  } finally {
    await browser.close();
  }
})();
