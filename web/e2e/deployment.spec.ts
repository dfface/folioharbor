import { expect, test } from "@playwright/test";

test("production Web serves SPA deep links and immutable fingerprinted assets with security headers", async ({ request }) => {
  const deepLink = await request.get(
    "/libraries/018f47b5-58b4-7ba6-9a3a-d9f41f17b001/items/018f47b5-58b4-7ba6-9a3a-d9f41f17c001/read",
  );
  expect(deepLink.status()).toBe(200);
  expect(deepLink.headers()["content-type"]).toContain("text/html");
  expect(deepLink.headers()["cache-control"]).toContain("no-store");
  expect(deepLink.headers()["content-security-policy"]).toContain("default-src 'self'");
  expect(deepLink.headers()["x-content-type-options"]).toBe("nosniff");

  const html = await deepLink.text();
  const assetPath = /<script[^>]+src="(\/assets\/[^"]+\.js)"/u.exec(html)?.[1];
  expect(assetPath, "the production index must reference a fingerprinted JavaScript asset").toBeDefined();

  const asset = await request.get(assetPath ?? "/missing-production-asset.js");
  expect(asset.status()).toBe(200);
  expect(asset.headers()["content-type"]).toContain("javascript");
  expect(asset.headers()["cache-control"]).toContain("immutable");
  expect(asset.headers()["cache-control"]).toContain("max-age=31536000");
  expect(asset.headers()["x-content-type-options"]).toBe("nosniff");
});
