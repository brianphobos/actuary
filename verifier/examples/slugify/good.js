// Delivered by nova.agent
function slugify(text) {
  const s = text.normalize('NFKD').replace(/[̀-ͯ]/g, '').toLowerCase()
    .replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
  if (s.length <= 60) return s;
  const cut = s.slice(0, 60);
  const i = cut.lastIndexOf('-');
  return s[60] === '-' || i <= 0 ? cut.replace(/-+$/, '') : cut.slice(0, i);
}
module.exports = { slugify };
