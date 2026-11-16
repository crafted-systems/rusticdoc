#!/usr/bin/env node
// Tells search engines that take IndexNow (Bing, Yandex, Seznam, Naver and
// others) which pages exist, so new and changed pages are crawled soon after a
// deploy instead of whenever the crawler next comes by. Bing's index also
// feeds ChatGPT search and Copilot.
//   node scripts/indexnow.mjs            every page in the live sitemap
//   node scripts/indexnow.mjs URL...     only these pages
// Run it after a deploy: the key file, public/6e5505df99d2626a7d96dc609b5ee8e5.txt, must be live
// first, because the search engines fetch it to check the request is ours.
const SITE = 'https://rusticdoc.com';
const KEY = '6e5505df99d2626a7d96dc609b5ee8e5';

async function sitemapUrls() {
  const index = await (await fetch(`${SITE}/sitemap-index.xml`)).text();
  const maps = [...index.matchAll(/<loc>([^<]+)<\/loc>/g)].map((m) => m[1]);
  const urls = [];
  for (const map of maps) {
    const xml = await (await fetch(map)).text();
    urls.push(...[...xml.matchAll(/<loc>([^<]+)<\/loc>/g)].map((m) => m[1]));
  }
  return urls;
}

const urlList = process.argv.length > 2 ? process.argv.slice(2) : await sitemapUrls();
const res = await fetch('https://api.indexnow.org/indexnow', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json; charset=utf-8' },
  body: JSON.stringify({ host: new URL(SITE).host, key: KEY, keyLocation: `${SITE}/${KEY}.txt`, urlList }),
});
// 200 and 202 both mean the list was accepted.
console.log(`${res.status} ${res.statusText}: sent ${urlList.length} URLs`);
process.exit(res.ok ? 0 : 1);
