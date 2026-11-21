const http = require('http');
const fs = require('fs');
const path = require('path');

const PORT = Number(process.env.PORT) || 8080;
const DIR = fs.realpathSync(__dirname);

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'application/javascript; charset=utf-8',
  '.wasm': 'application/wasm',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.png': 'image/png',
  '.ico': 'image/x-icon',
};

const server = http.createServer((req, res) => {
  const method = req.method || 'GET';
  if (method !== 'GET' && method !== 'HEAD') {
    res.writeHead(405, { 'Content-Type': 'text/plain', Allow: 'GET, HEAD' });
    res.end('405 Method Not Allowed');
    return;
  }

  const rawUrl = (req.url || '/').split('?')[0];
  let reqPath;
  try {
    reqPath = decodeURIComponent(rawUrl);
  } catch {
    res.writeHead(400, { 'Content-Type': 'text/plain' });
    res.end('400 Bad Request');
    return;
  }

  if (reqPath.includes('\0')) {
    res.writeHead(400, { 'Content-Type': 'text/plain' });
    res.end('400 Bad Request');
    return;
  }
  if (reqPath === '/' || reqPath === '') reqPath = '/index.html';

  // Normalize and enforce containment strictly inside DIR to block path traversal.
  const normalized = path.posix.normalize(reqPath).replace(/^\/+/, '');
  const candidate = path.resolve(DIR, normalized);
  if (candidate !== DIR && !candidate.startsWith(DIR + path.sep)) {
    res.writeHead(403, { 'Content-Type': 'text/plain' });
    res.end('403 Forbidden');
    return;
  }

  let realPath;
  try {
    realPath = fs.realpathSync(candidate);
  } catch {
    res.writeHead(404, { 'Content-Type': 'text/plain' });
    res.end('404 Not Found');
    return;
  }
  if (realPath !== DIR && !realPath.startsWith(DIR + path.sep)) {
    res.writeHead(403, { 'Content-Type': 'text/plain' });
    res.end('403 Forbidden');
    return;
  }

  let stat;
  try {
    stat = fs.statSync(realPath);
  } catch {
    res.writeHead(404, { 'Content-Type': 'text/plain' });
    res.end('404 Not Found');
    return;
  }

  let fileToServe = realPath;
  if (stat.isDirectory()) {
    const indexCandidate = path.join(realPath, 'index.html');
    try {
      const realIndex = fs.realpathSync(indexCandidate);
      if (realIndex !== DIR && !realIndex.startsWith(DIR + path.sep)) {
        res.writeHead(403, { 'Content-Type': 'text/plain' });
        res.end('403 Forbidden');
        return;
      }
      stat = fs.statSync(realIndex);
      if (!stat.isFile()) {
        res.writeHead(403, { 'Content-Type': 'text/plain' });
        res.end('403 Forbidden');
        return;
      }
      fileToServe = realIndex;
    } catch {
      res.writeHead(403, { 'Content-Type': 'text/plain' });
      res.end('403 Forbidden');
      return;
    }
  }

  // Never serve the dev server script itself.
  if (fileToServe === path.join(DIR, 'server.js')) {
    res.writeHead(403, { 'Content-Type': 'text/plain' });
    res.end('403 Forbidden');
    return;
  }

  serveFile(fileToServe, stat.size, method === 'HEAD', res);
});

function serveFile(filePath, byteLength, headOnly, res) {
  const ext = path.extname(filePath).toLowerCase();
  const contentType = MIME[ext] || 'application/octet-stream';
  const headers = {
    'Content-Type': contentType,
    'Content-Length': byteLength,
    'Cache-Control': 'no-cache',
    'X-Content-Type-Options': 'nosniff',
    'Cross-Origin-Resource-Policy': 'same-origin',
    'Cross-Origin-Opener-Policy': 'same-origin',
    'Cross-Origin-Embedder-Policy': 'require-corp',
  };
  if (headOnly) {
    res.writeHead(200, headers);
    res.end();
    return;
  }
  const stream = fs.createReadStream(filePath);
  stream.on('open', () => {
    res.writeHead(200, headers);
    stream.pipe(res);
  });
  stream.on('error', () => {
    if (!res.headersSent) {
      res.writeHead(500, { 'Content-Type': 'text/plain' });
      res.end('Internal Server Error');
    } else {
      res.destroy();
    }
  });
}

server.listen(PORT, '127.0.0.1', () => {
  console.log(`Server listening on http://127.0.0.1:${PORT}`);
});

