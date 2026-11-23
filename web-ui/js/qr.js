/* Standards-compliant ISO/IEC 18004 QR Code SVG encoder (extracted from app.js). */
(function (global) {
  'use strict';

  function renderPairingQrSvg(uri) {
    // Standards-compliant ISO/IEC 18004 QR Code encoder (Versions 1..6, Byte mode, ECC Level L, Mask 0)
    const dataBytes = Array.from(new TextEncoder().encode(uri));
    const VERSIONS = [
      null,
      { size: 21, total: 26, data: 19, ecc: 7, align: [] },
      { size: 25, total: 44, data: 34, ecc: 10, align: [6, 18] },
      { size: 29, total: 70, data: 55, ecc: 15, align: [6, 22] },
      { size: 33, total: 100, data: 80, ecc: 20, align: [6, 26] },
      { size: 37, total: 134, data: 108, ecc: 26, align: [6, 30] },
      { size: 41, total: 172, data: 136, ecc: 36, align: [6, 34] },
    ];
    let ver = 1;
    while (ver < VERSIONS.length && dataBytes.length + 2 > VERSIONS[ver].data) ver += 1;
    const spec = VERSIONS[Math.min(ver, VERSIONS.length - 1)];
    const payload = dataBytes.slice(0, spec.data - 2);

    // Bitstream: Mode 0100 (Byte), 8-bit length, payload bytes, terminator, pad bytes 0xEC / 0x11
    const bits = [];
    const pushBits = (val, len) => {
      for (let i = len - 1; i >= 0; i -= 1) bits.push((val >>> i) & 1);
    };
    pushBits(0b0100, 4);
    pushBits(payload.length, 8);
    for (const b of payload) pushBits(b, 8);
    pushBits(0, Math.min(4, spec.data * 8 - bits.length));
    while (bits.length % 8 !== 0) bits.push(0);
    const codewords = [];
    for (let i = 0; i < bits.length; i += 8) {
      let byte = 0;
      for (let j = 0; j < 8; j += 1) byte = (byte << 1) | bits[i + j];
      codewords.push(byte);
    }
    let padToggle = true;
    while (codewords.length < spec.data) {
      codewords.push(padToggle ? 0xec : 0x11);
      padToggle = !padToggle;
    }

    // GF(256) Reed-Solomon ECC (primitive polynomial 0x11D)
    const gfMul = (a, b) => {
      let r = 0;
      for (let i = 0; i < 8; i += 1) {
        if (b & 1) r ^= a;
        const hi = a & 0x80;
        a = (a << 1) & 0xff;
        if (hi) a ^= 0x1d;
        b >>>= 1;
      }
      return r;
    };
    let gen = [1];
    let root = 1;
    for (let i = 0; i < spec.ecc; i += 1) {
      const next = new Array(gen.length + 1).fill(0);
      for (let j = 0; j < gen.length; j += 1) {
        next[j] ^= gen[j];
        next[j + 1] ^= gfMul(gen[j], root);
      }
      gen = next;
      root = gfMul(root, 2);
    }
    const ecc = new Array(spec.ecc).fill(0);
    for (const b of codewords) {
      const factor = b ^ ecc.shift();
      ecc.push(0);
      for (let i = 0; i < spec.ecc; i += 1) ecc[i] ^= gfMul(gen[i + 1], factor);
    }
    const allCodewords = codewords.concat(ecc);

    const size = spec.size;
    const mat = Array.from({ length: size }, () => Array(size).fill(false));
    const isFunc = Array.from({ length: size }, () => Array(size).fill(false));
    const setFunc = (r, c, dark) => {
      if (r >= 0 && r < size && c >= 0 && c < size) {
        mat[r][c] = dark;
        isFunc[r][c] = true;
      }
    };
    const drawFinder = (r0, c0) => {
      for (let dr = -1; dr <= 7; dr += 1) {
        for (let dc = -1; dc <= 7; dc += 1) {
          const inBox = dr >= 0 && dr <= 6 && dc >= 0 && dc <= 6;
          const dark = inBox && (dr === 0 || dr === 6 || dc === 0 || dc === 6 || (dr >= 2 && dr <= 4 && dc >= 2 && dc <= 4));
          setFunc(r0 + dr, c0 + dc, dark);
        }
      }
    };
    drawFinder(0, 0);
    drawFinder(0, size - 7);
    drawFinder(size - 7, 0);
    for (let i = 8; i < size - 8; i += 1) {
      setFunc(6, i, i % 2 === 0);
      setFunc(i, 6, i % 2 === 0);
    }
    for (const ar of spec.align) {
      for (const ac of spec.align) {
        if ((ar === 6 && ac === 6) || (ar === 6 && ac === size - 7) || (ar === size - 7 && ac === 6)) continue;
        for (let dr = -2; dr <= 2; dr += 1) {
          for (let dc = -2; dc <= 2; dc += 1) {
            setFunc(ar + dr, ac + dc, Math.max(Math.abs(dr), Math.abs(dc)) !== 1);
          }
        }
      }
    }
    // Format info for ECC Level L (01) + Mask 0 (000) -> 0b01000 -> BCH(15,5) XOR 0x5412 = 0x77C4
    const fmt = 0x77c4;
    for (let i = 0; i <= 5; i += 1) setFunc(8, i, ((fmt >>> (14 - i)) & 1) === 1);
    setFunc(8, 7, ((fmt >>> 8) & 1) === 1);
    setFunc(8, 8, ((fmt >>> 7) & 1) === 1);
    setFunc(7, 8, ((fmt >>> 6) & 1) === 1);
    for (let i = 0; i <= 5; i += 1) setFunc(5 - i, 8, ((fmt >>> (5 - i)) & 1) === 1);
    for (let i = 0; i <= 7; i += 1) setFunc(8, size - 1 - i, ((fmt >>> i) & 1) === 1);
    for (let i = 0; i <= 6; i += 1) setFunc(size - 1 - i, 8, ((fmt >>> (14 - i)) & 1) === 1);
    setFunc(size - 8, 8, true); // Dark module

    // Place data bits zigzag right-to-left with Mask 0: (r + c) % 2 === 0
    let bitIdx = 0;
    const totalBits = allCodewords.length * 8;
    for (let right = size - 1; right >= 1; right -= 2) {
      if (right === 6) right = 5;
      const upward = (((right + 1) >>> 1) & 1) === 0;
      for (let step = 0; step < size; step += 1) {
        const r = upward ? size - 1 - step : step;
        for (let dx = 0; dx < 2; dx += 1) {
          const c = right - dx;
          if (isFunc[r][c]) continue;
          let dark = false;
          if (bitIdx < totalBits) {
            dark = ((allCodewords[bitIdx >>> 3] >>> (7 - (bitIdx & 7))) & 1) === 1;
            bitIdx += 1;
          }
          mat[r][c] = ((r + c) & 1) === 0 ? !dark : dark;
        }
      }
    }

    let rects = '';
    for (let r = 0; r < size; r += 1) {
      for (let c = 0; c < size; c += 1) {
        if (mat[r][c]) rects += `<rect x="${c + 2}" y="${r + 2}" width="1" height="1"/>`;
      }
    }
    const vb = size + 4;
    return `<svg viewBox="0 0 ${vb} ${vb}" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="配对二维码"><rect width="${vb}" height="${vb}" fill="#FFFFFF"/><g fill="#1E293B">${rects}</g></svg>`;
  }

  global.LanVault = global.LanVault || {};
  global.LanVault.qr = { renderPairingQrSvg };
})(window);
