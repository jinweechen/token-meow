'use strict';
// Native vector construction only. No character raster files are opened.
const fs=require('fs');
const path=require('path');
const sharp=require('C:/Users/Administrator/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/sharp');
const OUT=__dirname, W=1024, H=1536, GROUND=1483;
const HIP={x:600,y:840}, SHOULDER={x:600,y:620};
const THIGH=280, SHIN=280, UPPER_ARM=145, FOREARM=125;
const SHOE=[[-70,100],[-70,45],[-40,-10],[25,-10],[45,40],[125,55],[150,80],[150,100]];
const C={near:'#58c3b8',nearBone:'#176e67',far:'#b7bdc5',farBone:'#69747f'};
const round=n=>Number(n.toFixed(9));
const dist=(a,b)=>Math.hypot(a.x-b.x,a.y-b.y);
const rot=(p,degrees)=>{const a=degrees*Math.PI/180;return{x:p[0]*Math.cos(a)-p[1]*Math.sin(a),y:p[0]*Math.sin(a)+p[1]*Math.cos(a)};};
function ik(root,end,a,b,branch=1){
  const dx=end.x-root.x,dy=end.y-root.y,d=Math.hypot(dx,dy);
  if(!Number.isFinite(d)||d>a+b||d<Math.abs(a-b)||d===0)throw Error('Unreachable IK; no limb stretching');
  const along=(a*a-b*b+d*d)/(2*d),height=Math.sqrt(Math.max(0,a*a-along*along));
  return{x:root.x+dx*along/d+branch*dy*height/d,y:root.y+dy*along/d-branch*dx*height/d};
}
function foot(ankle,pitch){
  const points=SHOE.map(p=>{const v=rot(p,pitch);return{x:ankle.x+v.x,y:ankle.y+v.y};});
  return{ankle,pitchDegrees:pitch,heel:points[0],toe:points[points.length-1],polygon:points,lowestY:Math.max(...points.map(p=>p.y))};
}
function swingFoot(x,pitch,clearance){const localLowest=Math.max(...SHOE.map(p=>rot(p,pitch).y));return foot({x,y:GROUND-clearance-localLowest},pitch);}
function pivotFoot(toeX,pitch){const v=rot([150,100],pitch);return foot({x:toeX-v.x,y:GROUND-v.y},pitch);}
function leg(f,role){const knee=ik(HIP,f.ankle,THIGH,SHIN);const reach=dist(HIP,f.ankle);return{role,hip:{...HIP},knee,ankle:f.ankle,foot:f,upperLength:dist(HIP,knee),lowerLength:dist(knee,f.ankle),hipAnkleDistance:reach,kneeFlexionDegrees:180-Math.acos((THIGH*THIGH+SHIN*SHIN-reach*reach)/(2*THIGH*SHIN))*180/Math.PI,soleClearance:GROUND-f.lowestY};}
function arm(x,y){const wrist={x,y},elbow=ik(SHOULDER,wrist,UPPER_ARM,FOREARM,-1);return{shoulder:{...SHOULDER},elbow,wrist,upperLength:dist(SHOULDER,elbow),lowerLength:dist(elbow,wrist)};}
const bases=[
 {number:2,stem:'02-near-flat-recovery-guide',title:'02 · 近腿承重 / 远腿离地回收',near:leg(foot({x:630,y:1383},0),'全掌承重'),far:leg(swingFoot(550,4,35),'离地回收'),nearArm:arm(540,855),farArm:arm(660,855),note:'近脚全掌落地；远脚短距离回收，鞋底离地 35 px。'},
 {number:3,stem:'03-far-passing-guide',title:'03 · 近腿支撑 / 远脚经过',near:leg(foot({x:600,y:1383},0),'全掌支撑'),far:leg(swingFoot(600,0,50),'身体下方经过'),nearArm:arm(580,860),farArm:arm(620,860),note:'远脚从身体下经过，允许被近腿遮挡；鞋底离地 50 px。'},
 {number:4,stem:'04-near-toe-far-reach-guide',title:'04 · 近脚抬跟 / 远脚前摆',near:leg(pivotFoot(665,12),'前掌支点抬跟'),far:leg(swingFoot(660,-6,40),'前摆准备落地'),nearArm:arm(680,835),farArm:arm(520,835),note:'近脚绕前掌抬跟；远脚向前准备接触，鞋底仍离地 40 px。'}
];
const phases=[...bases,...bases.map(p=>({number:p.number+4,stem:({'2':'06-far-flat-recovery-guide','3':'07-near-passing-guide','4':'08-far-toe-near-reach-guide'})[p.number],title:({'2':'06 · 远腿承重 / 近腿离地回收','3':'07 · 远腿支撑 / 近脚经过','4':'08 · 远脚抬跟 / 近脚前摆'})[p.number],near:p.far,far:p.near,nearArm:p.farArm,farArm:p.nearArm,note:p.note.replaceAll('近','TEMP_NEAR').replaceAll('远','近').replaceAll('TEMP_NEAR','远'),oppositeOf:p.number}))];
const xy=p=>`${round(p.x)} ${round(p.y)}`;
const chain=(a,b,c)=>`M${xy(a)} L${xy(b)} L${xy(c)}`;
function volumes(p,side){const l=p[side],a=p[side+'Arm'],color=C[side];return `<g id="${side}-volumes" fill="${color}" stroke="${color}" stroke-linecap="round" stroke-linejoin="round"><path d="${chain(a.shoulder,a.elbow,a.wrist)}" fill="none" stroke-width="42"/><path d="${chain(l.hip,l.knee,l.ankle)}" fill="none" stroke-width="${side==='near'?98:90}"/><path d="M${l.foot.polygon.map(xy).join(' L')} Z" stroke-width="0"/></g>`;}
function bones(p,side){const l=p[side],a=p[side+'Arm'],col=C[side+'Bone'],dash=side==='far'?' stroke-dasharray="9 8"':'';return `<g id="${side}-bones" stroke="${col}" fill="none" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round"${dash}><path d="${chain(l.hip,l.knee,l.ankle)}"/><path d="${chain(a.shoulder,a.elbow,a.wrist)}" stroke-width="2"/><path d="${chain(l.ankle,l.foot.heel,l.foot.toe)}" stroke-width="2"/>${[l.knee,l.ankle].map(q=>`<circle cx="${q.x}" cy="${q.y}" r="8"/>`).join('')}</g>`;}
function svg(p){
 const moving=p.near.soleClearance>1?p.near:p.far,stance=p.near.soleClearance<1?p.near:p.far,clearance=round(moving.soleClearance);
 const nearLabel=p.near.role,farLabel=p.far.role;
 return `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">
 <title>${p.title}</title><desc>纯代码姿势约束，候选角色像素尺寸；不是人物素材或验收。固定近青远灰，远侧先画；虚线为被遮挡的连接。腿段 280/280 px，地面 y=1483。</desc>
 <rect width="1024" height="1536" fill="white"/>
 <g font-family="Microsoft YaHei, Noto Sans CJK SC, sans-serif" fill="#2c423a"><text x="55" y="76" font-size="30" font-weight="700">${p.title}</text><text x="55" y="122" font-size="20">${p.note}</text><text x="55" y="162" font-size="18" fill="#6c7a70">面向右 → · 固定画幅 1024×1536 · 地面 y=1483</text><text x="55" y="196" font-size="18" fill="#6c7a70">绘图约束，非人物成品或验收；不按成人工程比例重映射。</text>
 <rect x="55" y="238" width="22" height="22" rx="5" fill="${C.near}"/><text x="88" y="257" font-size="19">近侧：${nearLabel}</text><rect x="490" y="238" width="22" height="22" rx="5" fill="${C.far}"/><text x="523" y="257" font-size="19">远侧：${farLabel}</text>
 <text x="55" y="310" font-size="18" fill="#6c7a70">腿段均为 280 px；鞋底长 220 px。允许遮挡，不把远鞋移开求完整。</text><text x="55" y="342" font-size="18" fill="#6c7a70">灰虚线表示隐藏骨架；手臂随同侧腿反向摆动。</text></g>
 <path d="M600 570 V1483" stroke="#e3e8e4" stroke-width="1.5" stroke-dasharray="6 9"/>
 <path d="M60 1483 H965" stroke="#748176" stroke-width="3"/>
 ${volumes(p,'far')}
 <path d="M600 620 L600 840" stroke="#edf0ed" stroke-width="108" stroke-linecap="round"/>
 ${volumes(p,'near')}
 ${bones(p,'near')}${bones(p,'far')}
 <g fill="#fff" stroke="${C.nearBone}" stroke-width="3"><circle cx="600" cy="840" r="9"/><circle cx="600" cy="620" r="7"/></g>
 <g font-family="Microsoft YaHei, sans-serif" font-size="18" fill="#607267"><path d="M588 840 H324" stroke="#9aac9d" stroke-width="1.5"/><text x="110" y="846">髋 (600,840)</text><text x="110" y="881">固定腿长 280 / 280</text>
 <path d="M882 ${moving.foot.lowestY} H910 M882 1483 H910 M896 ${moving.foot.lowestY} V1483" stroke="#7c8b80" fill="none" stroke-width="1.5"/><text x="915" y="${moving.foot.lowestY+clearance/2+6}" font-size="16">${clearance}px</text>
 <text x="62" y="1517" font-size="17">原生 SVG 姿势约束 · 近青远灰固定层级 · 不作为人物通过证据</text></g>
 </svg>`;
}
async function main(){
 const checks=[];
 for(const p of phases){
  for(const side of ['near','far']){const l=p[side],a=p[side+'Arm'];if(Math.abs(l.upperLength-THIGH)>1e-8||Math.abs(l.lowerLength-SHIN)>1e-8||Math.abs(a.upperLength-UPPER_ARM)>1e-8||Math.abs(a.lowerLength-FOREARM)>1e-8)throw Error('segment error');if(l.soleClearance < -1e-8)throw Error('foot through ground');const branch=(l.knee.x-l.hip.x)*(l.ankle.y-l.hip.y)-(l.knee.y-l.hip.y)*(l.ankle.x-l.hip.x);if(branch<=0)throw Error('unexpected knee branch relative to hip-ankle line');}
  const source=svg(p);if(/<image\b|data:image|href\s*=/.test(source))throw Error('Raster input forbidden');
  fs.writeFileSync(path.join(OUT,p.stem+'.svg'),source+'\n');
  await sharp(Buffer.from(source),{density:72}).png().toFile(path.join(OUT,p.stem+'.png'));
  const info=await sharp(path.join(OUT,p.stem+'.png')).metadata();if(info.width!==W||info.height!==H)throw Error('wrong canvas');
  checks.push({phase:p.number,files:[p.stem+'.svg',p.stem+'.png'],legLengths:{near:[p.near.upperLength,p.near.lowerLength],far:[p.far.upperLength,p.far.lowerLength]},kneeFlexionDegrees:{near:p.near.kneeFlexionDegrees,far:p.far.kneeFlexionDegrees},clearance:{near:p.near.soleClearance,far:p.far.soleClearance},fixedCanvas:true,noRasterInput:true});
 }
 const doc={schemaVersion:1,purpose:'six-native-vector-character-pose-constraints',isCharacterMaterial:false,isCharacterAcceptance:false,isFullCycleAcceptance:false,coordinateSystem:'pixel coordinates, x right, y down',canvas:{width:W,height:H},groundY:GROUND,hip:HIP,legLengths:[THIGH,SHIN],shoeLocalPolygon:SHOE,shoeSoleLength:220,ankleToGroundAtFlat:100,drawOrder:['far complete limbs','torso volume','near complete limbs','near bones','far hidden bone dashes'],phases,checks,limitations:['Discrete drawing constraints; six phases do not validate an eight-frame animation or continuous contact.','Dimensions are chosen for this candidate character drawing context; not a remapping of the v9 metre model or a redefinition of the character appearance.','Opposite phases swap complete leg and arm geometry between fixed near/far identities; near remains front in draw order.','Capsules, hand endpoints and shoe polygons are diagram geometry, not finished anatomical contours.']};
 fs.writeFileSync(path.join(OUT,'phase-coordinates.json'),JSON.stringify(doc,null,2)+'\n');
 console.log(JSON.stringify(checks,null,2));
}
main().catch(e=>{console.error(e);process.exitCode=1;});
