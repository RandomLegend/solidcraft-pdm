// Photographs the server's administration page for the README (run by
// docs/make-server-screenshots.sh): node server-screenshots.js <chromium> <port> <out dir>
// <demo folder>. Paths in the demo folder are shown as a server would have them.
// The page is opened as http://pdm.workshop.lan:7380, the address a team would use.
"use strict";

const { chromium } = require("playwright-core");

const [executablePath, port, out, demo] = process.argv.slice(2);
const base = "http://pdm.workshop.lan:7380";

(async () => {
  const browser = await chromium.launch({
    executablePath,
    args: [`--host-resolver-rules=MAP pdm.workshop.lan:7380 127.0.0.1:${port}`],
  });
  const page = await browser.newPage({ viewport: { width: 1400, height: 860 }, deviceScaleFactor: 1.5, colorScheme: "dark" });
  const failures = [];
  page.on("pageerror", (e) => failures.push(String(e)));

  await page.goto(`${base}/`);
  await page.fill("input[name=user]", "admin");
  await page.fill("input[name=password]", "admin-password");
  await page.click("button[type=submit]");
  await page.waitForSelector(".cards");
  await page.screenshot({ path: `${out}/server-dashboard.png` });

  await page.goto(`${base}/admin/#/users`);
  // The users page itself, not the dashboard still on screen.
  await page.waitForSelector("h1:text-is('Users')");
  await page.waitForSelector("tbody tr");
  // Down to the table: the rest of the page is empty.
  const table = await page.locator(".panel").boundingBox();
  await page.screenshot({ path: `${out}/server-users.png`, clip: { x: 0, y: 0, width: 1400, height: Math.ceil(table.y + table.height + 32) } });

  await page.goto(`${base}/admin/#/backups`);
  await page.waitForSelector("form.settings");
  await page.evaluate((demo) => {
    const shown = { [`${demo}/backups`]: "/mnt/nas/pdm-backups", [`${demo}/pdm-data`]: "/srv/pdm" };
    for (const el of document.querySelectorAll("input, .path, .note, td span")) {
      for (const [from, to] of Object.entries(shown)) {
        if (el.value && el.value.includes(from)) el.value = el.value.replaceAll(from, to);
        if (el.children.length === 0 && el.textContent.includes(from)) el.textContent = el.textContent.replaceAll(from, to);
      }
    }
    // The demo keeps everything on one disk; a real share would not get that warning.
    for (const note of document.querySelectorAll(".note")) if (note.textContent.includes("same disk")) note.remove();
    document.querySelector(".notes:empty")?.remove();
  }, demo);
  await page.screenshot({ path: `${out}/server-backups.png`, fullPage: true });

  await browser.close();
  if (failures.length) {
    console.error(failures.join("\n"));
    process.exit(1);
  }
})();
