/* v9 rigid-foot roll engineering reference. No raster input or output. */
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
    heelRollEnd: 0.12,
    toeRollStart: 0.30,
    heelContactDegrees: 10,
    toeOffDegrees: 22,
    hipHeight: 0.905,
    bodyBob: 0.008,
    upperLeg: 0.43,
    lowerLeg: 0.43,
    ankleHeight: 0.065,
    footHeel: 0.07,
    footToe: 0.12,
    swingLift: 0.055,
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
    { phase: 0.000, name: '近脚跟接触', near: '以脚跟接触', far: '以前掌支点抬跟' },
    { phase: 0.120, name: '近脚平掌承重', near: '平掌支撑', far: '离地后摆动' },
    { phase: 0.300, name: '远脚通过', near: '支撑', far: '抬脚通过身体下方' },
    { phase: 0.450, name: '近脚抬跟 / 远脚前摆', near: '绕前掌支点抬跟', far: '准备脚跟接触' },
    { phase: 0.500, name: '远脚跟接触', near: '以前掌支点抬跟', far: '以脚跟接触' },
    { phase: 0.620, name: '远脚平掌承重', near: '离地后摆动', far: '平掌支撑' },
    { phase: 0.800, name: '近脚通过', near: '抬脚通过身体下方', far: '支撑' },
    { phase: 0.950, name: '远脚抬跟 / 近脚前摆', near: '准备脚跟接触', far: '绕前掌支点抬跟' }
  ]);
  const LIMITATIONS = Object.freeze([
    '骨架运动示意，非人物动画成品或验收；不为任何 PNG、裙装或既有动画提供自动通过证据。',
    '参数是二维工程参考值，不是本角色的实测人体比例，也不是临床步态模型。',
    '本工程骨架不是角色 Q 版比例；腿长和髋高不得用于重新定义角色外形或替代角色设计稿。',
    '足部是刚性二维脚跟—前掌线段。已实现脚跟接触、平掌、前掌支点抬跟和摆动；未模拟脚趾关节、鞋底弯曲、压力分布或真实力学。',
    '双腿在侧面投影中可以重叠；没有人为分开脚道，也没有用裙装遮住关节。',
    '近腿青色、远腿灰色的身份与绘制层级固定；左右解剖命名需另结合角色朝向设定。',
    '两腿理想对称，半周期轮廓可重复；各条腿分别沿连续轨迹运动，不是只把静止骨架换色。',
    '躯干是刚性体块，只加入小幅上下起伏；最高点位于支撑中段，未模拟骨盆扭转、肩胯反扭、肌肉、头发、裙摆或尾巴。',
    '膝前弯按髋踝连线的分支判断；膝的屏幕 x 可落在髋后方，这本身不等于反关节。',
    '连续模式由解析足部轨迹和双骨段 IK 求解；8 相位模式是离散研究展示，不是交付帧数建议。',
    '实际接触点按阶段选择脚跟、脚跟与前掌或前掌；踝点会随足部旋转，不能再用踝点固定作为无滑动条件。',
    '行进视野不足时镜头跟随；接触和滑动误差始终使用世界坐标计算。',
    '预览缓存轨迹并限制刷新频率只是降低重复计算。上轮 IAB 崩溃原因未定位，未声称已修复崩溃。'
  ]);
  const fraction = value => value - Math.floor(value);
  const smooth5 = u => u <= 0 ? 0 : u >= 1 ? 1 : Math.max(0, Math.min(1, u * u * u * (10 + u * (-15 + 6 * u))));
  const distance = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);
  const point = (x, y) => ({ x, y });
  function parameters(input) {
    const c = Object.assign({}, DEFAULTS, input || {});
    for (const [key, value] of Object.entries(c)) {
      if (!Number.isFinite(value)) throw Error('Parameter must be finite: ' + key);
    }
    for (const key of ['cycleSeconds', 'strideLength', 'hipHeight', 'upperLeg', 'lowerLeg', 'ankleHeight', 'footHeel', 'footToe', 'swingLift', 'torsoHeight', 'upperArm', 'lowerArm']) {
      if (c[key] <= 0) throw Error('Parameter must be positive: ' + key);
    }
    if (c.stanceFraction < 0.5 || c.stanceFraction > 0.75 || c.bodyBob < 0) throw Error('Unsupported stance or bob parameter');
    if (!(0 < c.heelRollEnd && c.heelRollEnd < c.toeRollStart && c.toeRollStart < c.stanceFraction)) throw Error('Foot phase boundaries must be ordered');
    if (!(c.heelContactDegrees > 0 && c.heelContactDegrees <= 40 && c.toeOffDegrees > 0 && c.toeOffDegrees <= 45)) throw Error('Unsupported foot roll angle');
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
  function footAt(time, offset, c) {
    const totalPhase = time / c.cycleSeconds;
    const shifted = totalPhase + offset, cycle = Math.floor(shifted);
    let q = fraction(shifted);
    for (const boundary of [c.heelRollEnd, c.toeRollStart, c.stanceFraction]) if (Math.abs(q - boundary) < 1e-12) q = boundary;
    const stepAhead = c.strideLength * c.stanceFraction / 2;
    const heelAnchorX = (cycle - offset) * c.strideLength + stepAhead - c.footHeel;
    const footLength = c.footHeel + c.footToe, toeAnchorX = heelAnchorX + footLength;
    const heelAngle = c.heelContactDegrees * Math.PI / 180, toeAngle = -c.toeOffDegrees * Math.PI / 180;
    let angle, heel, stage, swingProgress = null, contactNames;
    if (q < c.heelRollEnd) {
      stage = 'heel-contact'; contactNames = ['heel'];
      angle = heelAngle * (1 - smooth5(q / c.heelRollEnd));
      heel = point(heelAnchorX, 0);
    } else if (q < c.toeRollStart) {
      stage = 'flat'; contactNames = ['heel', 'toe']; angle = 0;
      heel = point(heelAnchorX, 0);
    } else if (q < c.stanceFraction) {
      stage = 'toe-pivot'; contactNames = ['toe'];
      angle = toeAngle * smooth5((q - c.toeRollStart) / (c.stanceFraction - c.toeRollStart));
      heel = point(toeAnchorX - footLength * Math.cos(angle), -footLength * Math.sin(angle));
    } else {
      const u = (q - c.stanceFraction) / (1 - c.stanceFraction);
      stage = 'swing'; contactNames = []; swingProgress = u;
      const blend = smooth5(u), bell = 64 * u * u * u * (1 - u) * (1 - u) * (1 - u);
      const startX = toeAnchorX - footLength * Math.cos(toeAngle), startY = -footLength * Math.sin(toeAngle);
      angle = toeAngle + (heelAngle - toeAngle) * blend;
      heel = point(startX + (heelAnchorX + c.strideLength - startX) * blend, startY * (1 - blend) + c.swingLift * bell);
    }
    const toe = point(heel.x + footLength * Math.cos(angle), heel.y + footLength * Math.sin(angle));
    const ankle = point(heel.x + c.footHeel * Math.cos(angle) - c.ankleHeight * Math.sin(angle), heel.y + c.footHeel * Math.sin(angle) + c.ankleHeight * Math.cos(angle));
    const contactPoints = contactNames.map(name => ({ name, point: name === 'heel' ? heel : toe, anchor: point(name === 'heel' ? heelAnchorX : toeAnchorX, 0) }));
    return { phase: q, cycle, stance: stage !== 'swing', stage, angleRadians: angle, angleDegrees: angle * 180 / Math.PI, swingProgress, heel, toe, ankle, contactPoints };
  }
  function legAt(time, offset, id, c, hip) {
    const foot = footAt(time, offset, c), ankle = foot.ankle;
    const solved = twoBone(hip, ankle, c.upperLeg, c.lowerLeg);
    return {
      id, phaseOffset: offset, ...foot,
      hip: point(hip.x, hip.y), knee: solved.knee,
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
    for (let i = 0; i < PHASES.length; i++) if (phase + 1e-12 >= PHASES[i].phase) index = i;
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
    let segmentError = 0, contactSpeed = 0, contactDrift = 0, ankleStanceSpeed = 0, footRigidError = 0, footAngleError = 0, rootSpeedError = 0, minimumKneeHipOffsetX = Infinity, minimumForwardBendOffset = Infinity, minimumSoleHeight = Infinity;
    let maximumLegReach = 0, maximumReachAt = null;
    const kneeFlexion = { all: [180, 0], stance: [180, 0], swing: [180, 0] };
    let allFinite = true, identityFixed = true, localContactSpeedError = 0, maximumSwingSoleClearance = 0;
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
        footRigidError = Math.max(footRigidError, Math.abs(distance(leg.heel, leg.toe) - c.footHeel - c.footToe), Math.abs(distance(leg.heel, leg.ankle) - Math.hypot(c.footHeel, c.ankleHeight)), Math.abs(distance(leg.toe, leg.ankle) - Math.hypot(c.footToe, c.ankleHeight)));
        footAngleError = Math.max(footAngleError, Math.abs(Math.atan2(leg.toe.y - leg.heel.y, leg.toe.x - leg.heel.x) - leg.angleRadians));
        if (!leg.stance) maximumSwingSoleClearance = Math.max(maximumSwingSoleClearance, Math.min(leg.heel.y, leg.toe.y));
        if (leg.stance && before.legs[id].stance && after.legs[id].stance) {
          ankleStanceSpeed = Math.max(ankleStanceSpeed, distance(after.legs[id].ankle, before.legs[id].ankle) / (2 * h));
        }
        for (const contact of leg.contactPoints) {
          contactDrift = Math.max(contactDrift, distance(contact.point, contact.anchor));
          if (!before.legs[id].contactPoints.some(p => p.name === contact.name) || !after.legs[id].contactPoints.some(p => p.name === contact.name)) continue;
          const a = before.legs[id][contact.name], b = after.legs[id][contact.name];
          contactSpeed = Math.max(contactSpeed, distance(a, b) / (2 * h));
          const localDx = ((b.x - after.root.x) - (a.x - before.root.x)) / (2 * h);
          localContactSpeedError = Math.max(localContactSpeedError, Math.abs(localDx + speed));
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
    const boundaryChecks = [];
    for (const [id, offset] of [['near', 0], ['far', 0.5]]) {
      for (const boundary of [0, c.heelRollEnd, c.toeRollStart, c.stanceFraction, 1]) {
        const t = (boundary - offset) * c.cycleSeconds;
        const f = localPoints(sample(t, c)), left = [1, 2, 3].map(i => localPoints(sample(t - i * h, c))), right = [1, 2, 3].map(i => localPoints(sample(t + i * h, c)));
        const position = Math.max(...f.map((v, i) => Math.abs(3 * left[0][i] - 3 * left[1][i] + left[2][i] - (3 * right[0][i] - 3 * right[1][i] + right[2][i]))));
        const velocity = Math.max(...f.map((v, i) => Math.abs((3 * v - 4 * left[0][i] + left[1][i] - (-3 * v + 4 * right[0][i] - right[1][i])) / (2 * h))));
        boundaryChecks.push({ id, phase: boundary, positionExtrapolationDifference: position, oneSidedVelocityDifference: velocity });
      }
    }
    let minimumEarlySwingToeHeight = Infinity;
    const earlySwingProbes = [1e-9, 1e-8, 1e-7, 1e-6, 1e-5, 1e-4, .001, .005, .01];
    for (const delta of earlySwingProbes) minimumEarlySwingToeHeight = Math.min(minimumEarlySwingToeHeight, sample((c.stanceFraction + delta) * c.cycleSeconds, c).legs.near.toe.y);
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
      rigidFootGeometry: footRigidError < 1e-9 && footAngleError < 1e-9,
      actualContactPointFixed: contactDrift < 1e-9 && contactSpeed < 1e-7,
      rootAndLocalContactMatch: localContactSpeedError < 1e-7,
      ankleMovesDuringFootRoll: ankleStanceSpeed > 1e-4,
      toeClearImmediatelyAfterToeOff: minimumEarlySwingToeHeight >= -1e-12,
      constantForwardSpeed: rootSpeedError < 1e-8,
      periodicLocalPositions: seamPosition < 1e-9,
      periodicLocalVelocitySeam: seamVelocity < 1e-7,
      noOneSidedVelocityJumpAtLoop: seamOneSidedVelocity < 1e-6,
      allFootPhaseBoundariesContinuous: boundaryChecks.every(b => b.positionExtrapolationDifference < 1e-8 && b.oneSidedVelocityDifference < 1e-6),
      nearFarIdentityFixed: identityFixed,
      sameSideArmOpposesLegAtContact: contactArmOpposition,
      eightDistinctPosesPerLeg: nearUnique === 8 && farUnique === 8,
      moreThanColorSwap: unlabelledUnique >= 4
    };
    return {
      scope: 'Deterministic engineering model only; no character artwork or animation acceptance',
      sampleCount: count + 1, finiteDifferenceStepSeconds: h, earlySwingProbeCount: earlySwingProbes.length, boundaryChecks,
      passed: Object.values(checks).every(Boolean), checks,
      metrics: {
        maximumSegmentLengthError: segmentError,
        maximumActualContactWorldSpeed: contactSpeed,
        maximumActualContactWorldDrift: contactDrift,
        maximumLocalContactSpeedMismatch: localContactSpeedError,
        maximumAnkleSpeedDuringStance: ankleStanceSpeed,
        maximumRigidFootDistanceError: footRigidError,
        maximumFootAngleError: footAngleError,
        minimumEarlySwingToeHeight,
        maximumSwingSoleClearance,
        maximumBoundaryPositionDifference: Math.max(...boundaryChecks.map(b => b.positionExtrapolationDifference)),
        maximumBoundaryVelocityDifference: Math.max(...boundaryChecks.map(b => b.oneSidedVelocityDifference)),
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
      modelRevision: 'rigid-foot-roll-study-v9-20261017',
      title: '小柚侧走 · 脚部滚动工程示意',
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
