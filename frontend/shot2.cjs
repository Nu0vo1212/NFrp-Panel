const { chromium } = require('playwright');
(async () => {
  const browser = await chromium.launch({ channel: 'msedge' });
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', e => errors.push('PAGEERROR: ' + e.message));
  await page.goto('http://127.0.0.1:8899/login', { waitUntil: 'networkidle' });
  await page.fill('input[type=text]', 'admin');
  await page.fill('input[type=password]', 'paneladmin123');
  await page.click('button.submit');
  await page.waitForURL('**/instances', { timeout: 15000 });
  await page.waitForTimeout(2000);
  await page.screenshot({ path: '../run/final_list.png' });

  // 进入详情
  await page.locator('tbody tr').first().click();
  await page.waitForTimeout(1800);
  await page.screenshot({ path: '../run/final_detail.png' });

  // 代理页签
  await page.locator('.tab', { hasText: '代理' }).click();
  await page.waitForTimeout(1200);
  await page.screenshot({ path: '../run/final_proxies.png' });

  console.log('ERRORS=' + JSON.stringify(errors));
  await browser.close();
})();