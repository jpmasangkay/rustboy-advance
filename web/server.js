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

  const stat = fs.statSync(realPath);
  if (stat.isDirectory()) {
    const indexPath = path.join(realPath, 'index.html');
    if (fs.existsSync(indexPath)) {
      serveFile(indexPath, res);
    } else {
      res.writeHead(403, { 'Content-Type': 'text/plain' });
      res.end('403 Forbidden');
    }
  } else {
    serveFile(realPath, res);
  }
});

function serveFile(filePath, res) {
  const ext = path.extname(filePath).toLowerCase();
  const contentType = MIME[ext] || 'application/octet-stream';
  fs.readFile(filePath, (err, data) => {
    if (err) {
      res.writeHead(500, { 'Content-Type': 'text/plain' });
      res.end('Internal Server Error');
      return;
    }
    res.writeHead(200, {
      'Content-Type': contentType,
      'Cache-Control': 'no-cache',
      'Cross-Origin-Opener-Policy': 'same-origin',
      'Cross-Origin-Embedder-Policy': 'require-corp',
    });
    res.end(data);
  });
}

server.listen(PORT, '127.0.0.1', () => {
  console.log(`Server listening on http://127.0.0.1:${PORT}`);
});

