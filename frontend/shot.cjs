const { chromium } = require('playwright');
(async () => {
  const browser = await chromium.launch({ channel: 'msedge' });
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await ctx.newPage();
  const errors = [];
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
  page.on('pageerror', e => errors.push('PAGEERROR: ' + e.message));

  await page.goto('http://127.0.0.1:8899/login', { waitUntil: 'networkidle' });
  await page.fill('input[type=text]', 'admin');
  await page.fill('input[type=password]', 'paneladmin123');
  await page.click('button.submit');
  await page.waitForURL('**/instances', { timeout: 15000 });
  await page.waitForTimeout(1800);
  await page.screenshot({ path: '../run/shot_instances.png' });

  const audit = page.locator('.nav-item', { hasText: '审计日志' });
  await audit.click();
  await page.waitForTimeout(150);
  await page.screenshot({ path: '../run/shot_nav_anim.png' });
  await page.waitForTimeout(700);
  await page.screenshot({ path: '../run/shot_audit.png' });

  await page.locator('.nav-item', { hasText: '设置' }).click();
  await page.waitForTimeout(900);
  await page.screenshot({ path: '../run/shot_settings.png' });

  await page.click('button.icon-btn');
  await page.waitForTimeout(450);
  await page.screenshot({ path: '../run/shot_dark.png' });

  console.log('CONSOLE_ERRORS=' + JSON.stringify(errors.slice(0, 6)));
  await browser.close();
})();