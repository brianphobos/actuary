// Builds dist/actuary.html: the dashboard with engine.js and swarm.js inlined, so it is a single file.
const fs = require('node:fs'), path = require('node:path');
let html = fs.readFileSync(path.join(__dirname, 'index.html'), 'utf8');
for (const f of ['engine.js', 'swarm.js', 'hive.js']) {
  const src = fs.readFileSync(path.join(__dirname, '..', 'sim', f), 'utf8');
  html = html.replace(`<script src="../sim/${f}"></script>`, () => `<script>\n${src}\n</script>`);
}
const artifact = process.argv.includes('--artifact');
if (artifact) html = html.split('\n').filter((l) => !l.includes('<!--local-only-->')).join('\n');
else html = html.replace(/<!--local-only-->/g, '');
fs.mkdirSync(path.join(__dirname, 'dist'), { recursive: true });
const out = path.join(__dirname, 'dist', artifact ? 'actuary-artifact.html' : 'actuary.html');
fs.writeFileSync(out, html);
console.log('wrote', path.relative(process.cwd(), out));
