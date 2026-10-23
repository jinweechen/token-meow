import test from "node:test";
import assert from "node:assert/strict";
import { mixtureStudyGeometry } from "./mixtureStudyGeometry";
import type { RollingProjection } from "./mixtureStudyContracts";
const start=Date.parse("2026-10-23T12:00:00.000Z"),at=(minute:number)=>new Date(start+minute*60000).toISOString();
function data():RollingProjection{return {quality:"conditional-manual",constraintsVerified:false,notifyEligible:false,uncertaintyBounds:null,asOf:at(0),endsAt:at(120),points:[{at:at(0),usedPoints:4,remainingPoints:6,available:true},{at:at(15),usedPoints:10,remainingPoints:0,available:false},{at:at(45),usedPoints:6,remainingPoints:4,available:true},{at:at(75),usedPoints:0,remainingPoints:10,available:true},{at:at(120),usedPoints:0,remainingPoints:10,available:true}],availabilityWindows:[],nextAvailableAt:at(0),reasons:[]};}
test("真实时间横轴与step-after保持释放阶梯，不按均匀桶或线性渐变伪造",()=>{
  const g=mixtureStudyGeometry(data(),10,{left:0,top:0,width:120,height:100});
  assert.equal(g.coordinates[1].x,15);assert.equal(g.coordinates[2].x,45);assert.equal(g.coordinates.at(-1)!.x,120);
  assert.equal(g.paths[0],"M 0 60 H 15 V 0 H 45 V 40 H 75 V 100 H 120 V 100");assert.ok(!g.paths[0].includes(" L "));
});
test("未知窗口断线且不落零轴，之后已知零与未知不同",()=>{
  const d=data();d.points[1]={at:at(15),usedPoints:null,remainingPoints:null,available:null};
  const g=mixtureStudyGeometry(d,10,{left:0,top:0,width:120,height:100});
  assert.equal(g.paths.length,2);assert.equal(g.paths[0],"M 0 60 H 15");assert.equal(g.coordinates[1].y,null);assert.equal(g.coordinates[3].y,100);assert.deepEqual(g.unknown,[{left:15,width:30}]);
});
