import assert from "node:assert/strict";
import test from "node:test";
import { demoSnapshot } from "./demo";
import {
  applyRootFilter,
  applySourceFilter,
  identityValue,
  parseIdentity,
  projectLabel,
  rootTurnOptions,
  turnsForSession,
} from "./filters";
import { exportResultMessage, parseExportResult } from "./exportResult";

test("root selection includes null/self roots, excludes known children and scopes identical short ids by source", () => {
  const snapshot = demoSnapshot();
  const root = snapshot.turns[0];
  const turns = [
    { ...root, sourceId: "source-a", id: "same", rootTurnId: null },
    { ...root, sourceId: "source-b", id: "same", rootTurnId: "same" },
    { ...root, sourceId: "source-a", id: "child", rootTurnId: "same" },
  ];
  const roots = rootTurnOptions(turns, []);
  assert.equal(roots.length, 2);
  assert.notEqual(identityValue(roots[0]), identityValue(roots[1]));
  assert.deepEqual(
    applyRootFilter(
      { sourceId: "source-a", project: "path:d:/one" },
      identityValue(roots[1]),
    ),
    { sourceId: "source-b", rootTurnId: "same" },
  );
  assert.deepEqual(
    applySourceFilter({ sourceId: "source-a", rootTurnId: "same" }, "source-b"),
    { sourceId: "source-b" },
  );
  assert.equal(parseIdentity("same"), null);
});
test("project identity stays stable when labels are private, same basename is not a lookup identity", () => {
  const snapshot = demoSnapshot();
  const session = {
    ...snapshot.sessions[0],
    project: "path:d:/one/app",
    projectName: "隐私模式",
  };
  const projects = [
    { id: "path:d:/one/app", name: "app" },
    { id: "path:e:/two/app", name: "app" },
  ];
  assert.equal(projectLabel(session, projects), "隐私模式");
  assert.equal(
    projectLabel({ ...session, projectName: undefined }, projects),
    "app",
  );
  assert.equal(new Set(projects.map((project) => project.id)).size, 2);
});
test("session details do not mix same thread ids from separate sources", () => {
  const snapshot = demoSnapshot();
  const session = { ...snapshot.sessions[0], sourceId: "source-a" };
  const turns = [
    { ...snapshot.turns[0], sourceId: "source-a", threadId: session.id },
    { ...snapshot.turns[0], sourceId: "source-b", threadId: session.id },
  ];
  assert.equal(turnsForSession(turns, session).length, 1);
  assert.equal(turnsForSession(turns, session)[0].sourceId, "source-a");
});
test("CSV cancel is silent, successful filename is explicit, object contract drift never claims success", () => {
  assert.equal(exportResultMessage(parseExportResult(null)), null);
  assert.equal(
    exportResultMessage(parseExportResult("用量.csv")),
    "已导出：用量.csv",
  );
  assert.throws(() => parseExportResult({ cancelled: true }), /未确认导出成功/);
  assert.throws(
    () => parseExportResult({ cancelled: false, fileName: "用量.csv" }),
    /未确认导出成功/,
  );
});
