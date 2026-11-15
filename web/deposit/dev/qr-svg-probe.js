// DEV ONLY. Run on dev/harness.html?s=handoff: decodes the QR the page actually rendered
// (the SVG in "Continue on your phone") with BarcodeDetector and compares it to the link.
(async () => {
  const svg = document.querySelector('details.handoff svg');
  if (!svg) return 'no svg';
  const xml = new XMLSerializer().serializeToString(svg).replace('<svg ', '<svg width="600" height="600" ');
  const img = new Image();
  img.src = 'data:image/svg+xml;base64,' + btoa(xml);
  await img.decode();
  const cv = document.createElement('canvas');
  cv.width = cv.height = 600;
  const g = cv.getContext('2d');
  g.fillStyle = '#fff';
  g.fillRect(0, 0, 600, 600);
  g.drawImage(img, 0, 0, 600, 600);
  const r = await new BarcodeDetector({ formats: ['qr_code'] }).detect(cv);
  const expected = location.origin + location.pathname + location.hash;
  return { decoded: r[0]?.rawValue, matches: r[0]?.rawValue === expected };
})()
