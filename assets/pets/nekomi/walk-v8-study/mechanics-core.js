/* Deterministic side-walk engineering reference. No raster input or output. */
(function (root, factory) {
  'use strict';
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.WalkMechanics = api;
})(typeof globalThis !== 'undefined' ? globalThis : this, function () {
  'use strict';
  const TAU = Math.PI * 2;
  const DEFAULTS = Object.freeze({
    cycleSeconds: 1.28,
    strideLength: 0.52,
    stanceFraction: 0.60,
    hipHeight: 0.905,
    bodyBob: 0.008,
    upperLeg: 0.43,
    lowerLeg: 0.43,
    ankleHeight: 0.065,
    footHeel: 0.07,
    footToe: 0.12,
    swingClearance: 0.075,
    torsoHeight: 0.34,
    torsoForward: 0.035,
    headAboveShoulder: 0.19,
    headForward: 0.015,
    headRadius: 0.10,
    upperArm: 0.27,
    lowerArm: 0.24,
    armSwingDegrees: 20,
    elbowFlexDegrees: 12
  });
  const PHASES = Object.freeze([
    { phase: 0.000, name: '近脚接触', near: '接触并承重', far: '支撑末段' },
    { phase: 0.125, name: '近脚承重', near: '支撑', far: '离地后摆动' },
    { phase: 0.300, name: '远脚通过', near: '支撑', far: '抬脚通过身体下方' },
    { phase: 0.375, name: '远脚前摆', near: '支撑后段', far: '向前伸出' },
    { phase: 0.500, name: '远脚接触', near: '支撑末段', far: '接触并承重' },
    { phase: 0.625, name: '远脚承重', near: '离地后摆动', far: '支撑' },
    { phase: 0.800, name: '近脚通过', near: '抬脚通过身体下方', far: '支撑' },
    { phase: 0.875, name: '近脚前摆', near: '向前伸出', far: '支撑后段' }
  ]);
  const LIMITATIONS = Object.freeze([
    '骨架运动示意，非人物动画成品或验收；不为任何 PNG、裙装或既有动画提供自动通过证据。',
    '参数是二维工程参考值，不是本角色的实测人体比例，也不是临床步态模型。',
    '本工程骨架不是角色 Q 版比例；腿长和髋高不得用于重新定义角色外形或替代角色设计稿。',
    '足部保持水平，未实现足跟落地、足掌滚动、脚趾蹬地或踝关节真实力学。',
    '双腿在侧面投影中可以重叠；没有人为分开脚道，也没有用裙装遮住关节。',
    '近腿青色、远腿灰色的身份与绘制层级固定；左右解剖命名需另结合角色朝向设定。',
    '两腿理想对称，半周期轮廓可重复；各条腿分别沿连续轨迹运动，不是只把静止骨架换色。',
    '躯干是刚性体块，只加入小幅上下起伏；最高点位于支撑中段，未模拟骨盆扭转、肩胯反扭、肌肉、头发、裙摆或尾巴。',
    '膝前弯按髋踝连线的分支判断；膝的屏幕 x 可落在髋后方，这本身不等于反关节。',
    '连续模式由解析足部轨迹和双骨段 IK 求解；8 相位模式是离散研究展示，不是交付帧数建议。',
    '行进视野不足时镜头跟随；接触和滑动误差始终使用世界坐标计算。'
  ]);
  const fraction = value => value - Math.floor(value);
  const smooth5 = u => u * u * u * (10 + u * (-15 + 6 * u));
  const distance = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);
  const point = (x, y) => ({ x, y });
  function parameters(input) {
    const c = Object.assign({}, DEFAULTS, input || {});
    for (const [key, value] of Object.entries(c)) {
      if (!Number.isFinite(value)) throw Error('Parameter must be finite: ' + key);
    }
    for (const key of ['cycleSeconds', 'strideLength', 'hipHeight', 'upperLeg', 'lowerLeg', 'ankleHeight', 'footHeel', 'footToe', 'swingClearance', 'torsoHeight', 'upperArm', 'lowerArm']) {
      if (c[key] <= 0) throw Error('Parameter must be positive: ' + key);
    }
    if (c.stanceFraction < 0.5 || c.stanceFraction > 0.75 || c.bodyBob < 0) throw Error('Unsupported stance or bob parameter');
    return c;
  }
  function twoBone(hip, ankle, upper, lower) {
    const dx = ankle.x - hip.x, dy = ankle.y - hip.y, d = Math.hypot(dx, dy);
    if (d <= Math.abs(upper - lower) + 1e-9 || d >= upper + lower - 1e-9) throw Error('Unreachable two-bone IK target; no silent limb stretching');
    const along = (upper * upper - lower * lower + d * d) / (2 * d);
    const bend = Math.sqrt(Math.max(0, upper * upper - along * along));
    // Ankle is below hip. This branch always bends toward screen-right.
    const knee = point(hip.x + along * dx / d - bend * dy / d, hip.y + along * dy / d + bend * dx / d);
    return { knee, bend, reach: d };
  }
  function legAt(time, offset, id, c, hip) {
    const totalPhase = time / c.cycleSeconds;
    const shifted = totalPhase + offset, q = fraction(shifted), cycle = Math.floor(shifted);
    const stepAhead = c.strideLength * c.stanceFraction / 2;
    const contactX = (cycle - offset) * c.strideLength + stepAhead;
    const stance = q < c.stanceFraction;
    let x = contactX, lift = 0, swingProgress = null;
    if (!stance) {
      const u = (q - c.stanceFraction) / (1 - c.stanceFraction);
      swingProgress = u;
      x += c.strideLength * smooth5(u);
      // Zero first and second derivatives at both lift endpoints.
      lift = c.swingClearance * 64 * u * u * u * (1 - u) * (1 - u) * (1 - u);
    }
    const ankle = point(x, c.ankleHeight + lift);
    const solved = twoBone(hip, ankle, c.upperLeg, c.lowerLeg);
    return {
      id, phaseOffset: offset, phase: q, cycle, stance, swingProgress,
      hip: point(hip.x, hip.y), knee: solved.knee, ankle,
      heel: point(x - c.footHeel, lift), toe: point(x + c.footToe, lift),
      contactWorld: stance ? point(contactX, 0) : null,
      ikBend: solved.bend,
      errors: {
        upperLength: distance(hip, solved.knee) - c.upperLeg,
        lowerLength: distance(solved.knee, ankle) - c.lowerLeg
      }
    };
  }
  function armAt(shoulder, phase, id, c) {
    const angle = -c.armSwingDegrees * Math.PI / 180 * Math.cos(TAU * phase);
    const foreAngle = angle + c.elbowFlexDegrees * Math.PI / 180;
    const elbow = point(shoulder.x + c.upperArm * Math.sin(angle), shoulder.y - c.upperArm * Math.cos(angle));
    const wrist = point(elbow.x + c.lowerArm * Math.sin(foreAngle), elbow.y - c.lowerArm * Math.cos(foreAngle));
    return { id, shoulder: point(shoulder.x, shoulder.y), elbow, wrist, upperAngle: angle, foreAngle };
  }
  function sample(time, input) {
    const c = parameters(input);
    if (!Number.isFinite(time)) throw Error('Time must be finite');
    const totalPhase = time / c.cycleSeconds, phase = fraction(totalPhase);
    // Raise the pelvis around mid-stance rather than keeping a low crouched arc.
    const root = point(c.strideLength * totalPhase, c.hipHeight + c.bodyBob * Math.cos(2 * TAU * (phase - c.stanceFraction / 2)));
    const rootVelocity = point(c.strideLength / c.cycleSeconds, -c.bodyBob * 2 * TAU / c.cycleSeconds * Math.sin(2 * TAU * (phase - c.stanceFraction / 2)));
    const bodyVerticalState = Math.abs(rootVelocity.y) < 1e-10 ? (root.y >= c.hipHeight ? 'apex' : 'trough') : (rootVelocity.y > 0 ? 'rising' : 'falling');
    const shoulder = point(root.x + c.torsoForward, root.y + c.torsoHeight);
    const near = legAt(time, 0, 'near', c, root), far = legAt(time, 0.5, 'far', c, root);
    return {
      time, totalPhase, phase, cycle: Math.floor(totalPhase), root, rootVelocity, bodyVerticalState,
      rootSpeed: c.strideLength / c.cycleSeconds,
      shoulder, head: point(shoulder.x + c.headForward, shoulder.y + c.headAboveShoulder),
      legs: { near, far },
      arms: { near: armAt(shoulder, near.phase, 'near', c), far: armAt(shoulder, far.phase, 'far', c) }
    };
  }
  function localPoints(frame) {
    const values = [];
    const add = p => values.push(p.x - frame.root.x, p.y);
    add(frame.root); add(frame.shoulder); add(frame.head);
    for (const id of ['near', 'far']) {
      for (const name of ['hip', 'knee', 'ankle', 'heel', 'toe']) add(frame.legs[id][name]);
      for (const name of ['shoulder', 'elbow', 'wrist']) add(frame.arms[id][name]);
    }
    return values;
  }
  function frameIndex(phase) {
    let index = 0;
    for (let i = 0; i < PHASES.length; i++) if (phase >= PHASES[i].phase) index = i;
    return index;
  }
  function discreteSample(time, input) {
    const c = parameters(input), total = time / c.cycleSeconds;
    return sample((Math.floor(total) + PHASES[frameIndex(fraction(total))].phase) * c.cycleSeconds, c);
  }
  function numericFinite(value) {
    if (typeof value === 'number') return Number.isFinite(value);
    if (Array.isArray(value)) return value.every(numericFinite);
    if (value && typeof value === 'object') return Object.values(value).every(numericFinite);
    return true;
  }
  function validate(input) {
    const c = parameters(input), count = 1200, h = 1e-5;
    let segmentError = 0, stanceSpeed = 0, rootSpeedError = 0, minimumKneeHipOffsetX = Infinity, minimumForwardBendOffset = Infinity, minimumSoleHeight = Infinity;
    let maximumLegReach = 0, maximumReachAt = null;
    const kneeFlexion = { all: [180, 0], stance: [180, 0], swing: [180, 0] };
    let allFinite = true, identityFixed = true, localStanceSpeedError = 0;
    const speed = c.strideLength / c.cycleSeconds;
    for (let i = 0; i <= count; i++) {
      const t = i / count * c.cycleSeconds, f = sample(t, c);
      allFinite = allFinite && numericFinite(f);
      const before = sample(t - h, c), after = sample(t + h, c);
      rootSpeedError = Math.max(rootSpeedError, Math.abs((after.root.x - before.root.x) / (2 * h) - speed));
      for (const id of ['near', 'far']) {
        const leg = f.legs[id], a = f.arms[id];
        identityFixed = identityFixed && leg.id === id && a.id === id && leg.phaseOffset === (id === 'near' ? 0 : 0.5);
        segmentError = Math.max(segmentError, Math.abs(leg.errors.upperLength), Math.abs(leg.errors.lowerLength), Math.abs(distance(a.shoulder, a.elbow) - c.upperArm), Math.abs(distance(a.elbow, a.wrist) - c.lowerArm));
        minimumKneeHipOffsetX = Math.min(minimumKneeHipOffsetX, leg.knee.x - leg.hip.x);
        const dx = leg.ankle.x - leg.hip.x, dy = leg.ankle.y - leg.hip.y, reach = Math.hypot(dx, dy);
        const signedFrontBend = ((leg.knee.x - leg.hip.x) * -dy + (leg.knee.y - leg.hip.y) * dx) / reach;
        minimumForwardBendOffset = Math.min(minimumForwardBendOffset, signedFrontBend);
        const internalCosine = (c.upperLeg * c.upperLeg + c.lowerLeg * c.lowerLeg - reach * reach) / (2 * c.upperLeg * c.lowerLeg);
        const flexion = 180 - Math.acos(Math.max(-1, Math.min(1, internalCosine))) * 180 / Math.PI;
        for (const group of ['all', leg.stance ? 'stance' : 'swing']) {
          kneeFlexion[group][0] = Math.min(kneeFlexion[group][0], flexion);
          kneeFlexion[group][1] = Math.max(kneeFlexion[group][1], flexion);
        }
        if (reach > maximumLegReach) { maximumLegReach = reach; maximumReachAt = { id, phase: leg.phase, time: t }; }
        minimumSoleHeight = Math.min(minimumSoleHeight, leg.heel.y, leg.toe.y);
        if (leg.stance && leg.phase > h / c.cycleSeconds && leg.phase < c.stanceFraction - h / c.cycleSeconds) {
          const dx = (after.legs[id].ankle.x - before.legs[id].ankle.x) / (2 * h);
          const dy = (after.legs[id].ankle.y - before.legs[id].ankle.y) / (2 * h);
          stanceSpeed = Math.max(stanceSpeed, Math.hypot(dx, dy));
          const localDx = ((after.legs[id].ankle.x - after.root.x) - (before.legs[id].ankle.x - before.root.x)) / (2 * h);
          localStanceSpeedError = Math.max(localStanceSpeedError, Math.abs(localDx + speed));
        }
      }
    }
    const start = localPoints(sample(0, c)), end = localPoints(sample(c.cycleSeconds, c));
    const startBefore = localPoints(sample(-h, c)), startAfter = localPoints(sample(h, c));
    const startBefore2 = localPoints(sample(-2 * h, c)), startAfter2 = localPoints(sample(2 * h, c));
    const endBefore = localPoints(sample(c.cycleSeconds - h, c)), endAfter = localPoints(sample(c.cycleSeconds + h, c));
    const seamPosition = Math.max(...start.map((v, i) => Math.abs(v - end[i])));
    const seamVelocity = Math.max(...start.map((v, i) => Math.abs((startAfter[i] - startBefore[i] - endAfter[i] + endBefore[i]) / (2 * h))));
    const seamOneSidedVelocity = Math.max(...start.map((v, i) => Math.abs((-3 * v + 4 * startAfter[i] - startAfter2[i] - (3 * v - 4 * startBefore[i] + startBefore2[i])) / (2 * h))));
    const keyframes = PHASES.map(p => sample(p.phase * c.cycleSeconds, c));
    const signature = (f, id) => ['hip', 'knee', 'ankle', 'heel', 'toe'].flatMap(name => [f.legs[id][name].x - f.root.x, f.legs[id][name].y]).map(n => n.toFixed(8)).join(',');
    const nearUnique = new Set(keyframes.map(f => signature(f, 'near'))).size;
    const farUnique = new Set(keyframes.map(f => signature(f, 'far'))).size;
    const unlabelledUnique = new Set(keyframes.map(f => [signature(f, 'near'), signature(f, 'far')].sort().join('|'))).size;
    const contactArmOpposition = sample(0, c).arms.near.wrist.x < sample(0, c).shoulder.x && sample(0, c).arms.far.wrist.x > sample(0, c).shoulder.x;
    const checks = {
      finiteCoordinates: allFinite,
      constantSegmentLengths: segmentError < 1e-9,
      kneesOnAnatomicalForwardBendBranch: minimumForwardBendOffset > 0,
      positiveExtensionMargin: c.upperLeg + c.lowerLeg - maximumLegReach > 1e-6,
      noSoleBelowGround: minimumSoleHeight >= -1e-12,
      stanceWorldContactFixed: stanceSpeed < 1e-8,
      rootAndLocalStanceMatch: localStanceSpeedError < 1e-8,
      constantForwardSpeed: rootSpeedError < 1e-8,
      periodicLocalPositions: seamPosition < 1e-9,
      periodicLocalVelocitySeam: seamVelocity < 1e-7,
      noOneSidedVelocityJumpAtLoop: seamOneSidedVelocity < 1e-7,
      nearFarIdentityFixed: identityFixed,
      sameSideArmOpposesLegAtContact: contactArmOpposition,
      eightDistinctPosesPerLeg: nearUnique === 8 && farUnique === 8,
      moreThanColorSwap: unlabelledUnique >= 4
    };
    return {
      scope: 'Deterministic engineering model only; no character artwork or animation acceptance',
      sampleCount: count + 1, finiteDifferenceStepSeconds: h,
      passed: Object.values(checks).every(Boolean), checks,
      metrics: {
        maximumSegmentLengthError: segmentError,
        maximumStanceWorldSpeed: stanceSpeed,
        maximumLocalStanceSpeedMismatch: localStanceSpeedError,
        maximumRootSpeedError: rootSpeedError,
        maximumLocalCyclePositionSeam: seamPosition,
        maximumLocalCycleVelocitySeam: seamVelocity,
        maximumOneSidedVelocityJumpAtLoop: seamOneSidedVelocity,
        minimumAnatomicalForwardBendOffset: minimumForwardBendOffset,
        minimumKneeRelativeToHipX: minimumKneeHipOffsetX,
        kneeFlexionDegrees: kneeFlexion,
        maximumLegReach,
        maximumLegExtensionRatio: maximumLegReach / (c.upperLeg + c.lowerLeg),
        minimumRemainingExtension: c.upperLeg + c.lowerLeg - maximumLegReach,
        maximumKneeExtensionDegrees: 180 - kneeFlexion.all[0],
        maximumReachAt,
        minimumSoleHeight,
        nearDistinctKeyPoses: nearUnique, farDistinctKeyPoses: farUnique,
        distinctUnlabelledKeyGeometry: unlabelledUnique
      }
    };
  }
  function dataset(input, includeDense) {
    const c = parameters(input);
    const data = {
      schemaVersion: 1,
      modelRevision: 'upright-short-stride-study-20261017',
      title: '小柚侧走 · 骨架运动工程示意',
      status: 'technical-study-only-not-character-acceptance',
      units: 'metres and seconds; arbitrary engineering scale, not measured character anatomy',
      coordinateSystem: 'x forward/right, y upward; ground y=0; root x advances continuously',
      drawOrder: ['far-arm', 'far-leg', 'torso', 'near-leg', 'near-arm'],
      identity: { near: { color: '#087f99', phaseOffset: 0 }, far: { color: '#808a93', phaseOffset: 0.5 } },
      parameters: c, keyPhases: PHASES,
      limitations: LIMITATIONS,
      keyframes: PHASES.map((p, i) => Object.assign({ keyIndex: i, name: p.name }, sample(p.phase * c.cycleSeconds, c))),
      validation: validate(c)
    };
    if (includeDense) data.continuousSamples = Array.from({ length: 257 }, (_, i) => sample(i / 256 * c.cycleSeconds, c));
    return data;
  }
  return { DEFAULTS, PHASES, LIMITATIONS, parameters, sample, discreteSample, frameIndex, validate, dataset, localPoints };
});
