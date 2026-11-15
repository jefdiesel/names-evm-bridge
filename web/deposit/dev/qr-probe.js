// DEV ONLY. Run in the page (dev/shoot.mjs --eval-file dev/qr-probe.js --url http://127.0.0.1:PORT/index.html):
// encodes test strings with src/lib/qr.js and decodes them with the browser's BarcodeDetector.
(async () => {
  const { encodeQr } = await import('/src/lib/qr.js');
  const V = { valid: [{ string: 'logosdep1qyqqqqqqqz4rdfuv3vnxfh3gu558zkxmfkm963yx8zr3kaqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqu3cdun0cyqqp930ngllc3rfp2kg60gw4zhjflmrk494scj9xnqsmmjxkyualkc02axs7z' }, { string: 'logosdep1qyqqqqqqqz4rdfuv3vnxfh3gu558zkxmfkm963yx8zr3ka9vypaehl4l8mmuyc5haf4t4hhve392w8sqqszltcgqcv23qjd4xcv5ay6m7fsa37zpy3yjrl3gj2pxvcersvnnvt9nm0esssqx5l' }, null, { string: 'logosdep1qyqqqqqqqqqqqq0lllllllllllllllllllllllllllllllcqqqqqqqqqqqqqqqqqqqqqqqqqqqqqplllyrlllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllln99l9p' }] };
  const inputs = [
    'HELLO WORLD',
    'hello',
    'http://127.0.0.1:8731/#' + (V ? V.valid[0].string : ''),
    V ? V.valid[3].string.toUpperCase() : 'X',
    V.valid[1].string,
    'https://deposit.example.org/logos/bridge/deposit/#' + (V ? V.valid[3].string : ''),
    'x'.repeat(300),
  ];
  const det = new BarcodeDetector({ formats: ['qr_code'] });
  const out = [];
  for (const text of inputs) {
    for (const ecl of ['M', 'L']) {
      const qr = encodeQr(text, { ecl });
      const scale = 6, n = (qr.size + 8) * scale;
      const cv = document.createElement('canvas'); cv.width = cv.height = n;
      const g = cv.getContext('2d'); g.fillStyle = '#fff'; g.fillRect(0, 0, n, n); g.fillStyle = '#000';
      for (let y = 0; y < qr.size; y++) for (let x = 0; x < qr.size; x++) if (qr.modules[y][x]) g.fillRect((x + 4) * scale, (y + 4) * scale, scale, scale);
      const r = await det.detect(cv);
      out.push({ len: text.length, ecl, version: qr.version, mask: qr.mask, ok: r.length === 1 && r[0].rawValue === text, got: r[0]?.rawValue?.slice(0, 40) });
    }
  }
  return out;
})()
