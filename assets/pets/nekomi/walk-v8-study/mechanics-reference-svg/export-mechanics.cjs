/* Fixed-frame SVG engineering export. Does not read or write character PNGs. */
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const base = path.resolve(__dirname, '..');
const context = {};
vm.runInNewContext(fs.readFileSync(path.join(base, 'mechanics-core.js'), 'utf8'), context);
const model = context.WalkMechanics;
const data = model.dataset();
if (!data.validation.passed) throw Error('Model numeric constraints failed; SVG export stopped');
const c = data.parameters;
const SIZE = 768, SCALE = 400, FLOOR = 700, ORIGIN = 384;
const color = { near: '#087f99', far: '#808a93' };
const vertical = { rising: '身体上升', falling: '身体下降', apex: '身体最高点', trough: '身体最低点' };
const phaseNames = ['near-contact', 'near-support', 'far-passing', 'far-forward-swing', 'far-contact', 'far-support', 'near-passing', 'near-forward-swing'];
const esc = text => String(text).replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;');
function draw(frame, index) {
  const x = p => ORIGIN + (p.x - frame.root.x) * SCALE, y = p => FLOOR - p.y * SCALE;
  const n = value => value.toFixed(3), xy = p => `${n(x(p))},${n(y(p))}`;
  const line = (points, attributes) => `<polyline points="${points.map(xy).join(' ')}" fill="none" ${attributes}/>`;
  const circle = (p, radius, fill, stroke = 'none', width = 1) => `<circle cx="${n(x(p))}" cy="${n(y(p))}" r="${radius}" fill="${fill}" stroke="${stroke}" stroke-width="${width}"/>`;
  const label = (p, value, dx, dy, ink, anchor = 'start') => `<text x="${n(x(p) + dx)}" y="${n(y(p) + dy)}" fill="${ink}" text-anchor="${anchor}" font-size="14" paint-order="stroke" stroke="#fff" stroke-width="4">${esc(value)}</text>`;
  const flex = l => {
    const d = Math.hypot(l.ankle.x - l.hip.x, l.ankle.y - l.hip.y);
    return 180 - Math.acos(Math.max(-1, Math.min(1, (c.upperLeg ** 2 + c.lowerLeg ** 2 - d ** 2) / (2 * c.upperLeg * c.lowerLeg)))) * 180 / Math.PI;
  };
  let content = `<rect width="768" height="768" fill="#fff"/><g font-family="system-ui,Microsoft YaHei,sans-serif"><text x="24" y="29" fill="#263e34" font-size="21" font-weight="650">${index + 1}. ${esc(data.keyPhases[index].name)} · ${(frame.phase * 100).toFixed(1)}%</text><text x="24" y="53" fill="#65776a" font-size="13">${vertical[frame.bodyVerticalState]} · 骨架运动参考，非人物成品或验收</text>`;
  for (let local = -0.75; local <= 0.75; local += 0.25) {
    const px = ORIGIN + local * SCALE;
    content += `<line x1="${px}" y1="72" x2="${px}" y2="700" stroke="#e9eee9"/><text x="${px}" y="725" text-anchor="middle" font-size="11" fill="#849185">${local.toFixed(2)} m</text>`;
  }
  content += '<line x1="28" y1="700" x2="740" y2="700" stroke="#a9b8a9" stroke-width="2"/>';
  const arm = id => {
    const a = frame.arms[id];
    return line([a.shoulder, a.elbow, a.wrist], `stroke="${color[id]}" stroke-width="11" stroke-linecap="round" stroke-linejoin="round"`) + circle(a.elbow, 5, '#fff', color[id], 2) + circle(a.wrist, 6, color[id]);
  };
  const leg = id => {
    const l = frame.legs[id];
    let out = line([l.hip, l.knee, l.ankle], `stroke="${color[id]}" stroke-width="16" stroke-linecap="round" stroke-linejoin="round"`);
    out += line([l.ankle, l.heel, l.toe, l.ankle], `stroke="${color[id]}" stroke-width="4" stroke-linejoin="round"`);
    for (const key of ['hip', 'knee', 'ankle']) out += circle(l[key], 6, '#fff', color[id], 2);
    for (const key of ['heel', 'toe']) out += circle(l[key], 3.5, color[id]);
    if (l.stance) out += line([{ x: l.heel.x, y: -0.012 }, { x: l.toe.x, y: -0.012 }], `stroke="${color[id]}" stroke-width="3"`);
    return out;
  };
  content += arm('far') + leg('far');
  content += line([frame.root, frame.shoulder], 'stroke="#d8dfcf" stroke-width="55" stroke-linecap="round"');
  content += line([frame.root, frame.shoulder], 'stroke="#92a382" stroke-width="2" stroke-dasharray="5 5"');
  content += circle(frame.head, c.headRadius * SCALE, '#e8ecdf', '#8e9f7f', 2);
  content += line([{ x: frame.head.x + c.headRadius * 0.8, y: frame.head.y + 0.025 }, { x: frame.head.x + c.headRadius + 0.025, y: frame.head.y - 0.01 }, { x: frame.head.x + c.headRadius * 0.8, y: frame.head.y - 0.025 }], 'stroke="#8e9f7f" stroke-width="2"');
  content += circle({ x: frame.head.x + c.headRadius * 0.45, y: frame.head.y + 0.025 }, 3, '#6d7c61');
  content += leg('near') + arm('near') + circle(frame.root, 8, '#fff', '#697b5e', 2);
  for (const id of ['far', 'near']) {
    const l = frame.legs[id], prefix = id === 'near' ? 'N' : 'F', side = id === 'near' ? 1 : -1;
    for (const [key, name] of [['hip', '髋'], ['knee', '膝'], ['ankle', '踝'], ['heel', '跟'], ['toe', '趾']]) {
      const dy = key === 'heel' ? 18 : key === 'toe' ? -14 : -12;
      content += label(l[key], `${prefix} ${name}`, side * 13, dy, color[id], side > 0 ? 'start' : 'end');
    }
  }
  content += `<text x="24" y="96" fill="${color.near}" font-size="14">N 近腿 · ${frame.legs.near.stance ? '支撑' : '摆动'}</text><text x="24" y="117" fill="${color.far}" font-size="14">F 远腿 · ${frame.legs.far.stance ? '支撑' : '摆动'}</text><text x="24" y="149" fill="#68796b" font-size="13">膝屈曲 N ${flex(frame.legs.near).toFixed(1)}° / F ${flex(frame.legs.far).toFixed(1)}°</text><text x="24" y="170" fill="#68796b" font-size="13">t=${frame.time.toFixed(3)} s · 髋高 ${frame.root.y.toFixed(3)} m</text><text x="24" y="748" fill="#697a6c" font-size="12">固定画幅 768×768；共享原地坐标。工程比例不得替代角色 Q 版设计；未做足部滚动。</text></g>`;
  return content;
}
const files = data.keyframes.map((frame, i) => {
  const file = `${String(i + 1).padStart(2, '0')}-${phaseNames[i]}.svg`;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${SIZE}" height="${SIZE}" viewBox="0 0 ${SIZE} ${SIZE}" role="img" aria-label="${esc(data.keyPhases[i].name)}，骨架技术参考"><title>${esc(data.keyPhases[i].name)} · 骨架技术参考</title>${draw(frame, i)}</svg>\n`;
  fs.writeFileSync(path.join(__dirname, file), svg);
  return { path: file, phase: frame.phase, time: frame.time, name: data.keyPhases[i].name, bodyVerticalState: frame.bodyVerticalState, rootVerticalSpeed: frame.rootVelocity.y };
});
const sheet = `<svg xmlns="http://www.w3.org/2000/svg" width="3072" height="1536" viewBox="0 0 3072 1536"><title>8 相位固定画幅骨架工程参考，不是人物动画素材</title>${data.keyframes.map((f, i) => `<svg x="${i % 4 * SIZE}" y="${Math.floor(i / 4) * SIZE}" width="768" height="768" viewBox="0 0 768 768">${draw(f, i)}</svg>`).join('')}</svg>\n`;
fs.writeFileSync(path.join(__dirname, 'eight-phase-reference.svg'), sheet);
fs.writeFileSync(path.join(base, 'mechanics.json'), JSON.stringify(data, null, 2) + '\n');
fs.writeFileSync(path.join(__dirname, 'reference-index.json'), JSON.stringify({ schemaVersion: 1, status: 'engineering-reference-only-not-character-art', canvas: [SIZE, SIZE], fixedScalePixelsPerMetre: SCALE, fixedGroundY: FLOOR, fixedRootX: ORIGIN, coordinateTransform: 'same source-to-SVG scale; remove overall root x only for in-place viewing; no per-frame bounds normalization', modelRevision: data.modelRevision, files, contactSheet: 'eight-phase-reference.svg', validation: data.validation }, null, 2) + '\n');
console.log(JSON.stringify({ files: files.map(x => x.path), contactSheet: 'eight-phase-reference.svg', numericChecksPassed: data.validation.passed, metrics: data.validation.metrics }, null, 2));
