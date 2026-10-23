import type { RollingProjection } from "./mixtureStudyContracts";
import { usageTrendValue } from "./usageTrendGeometry";

/** 仅为 SVG 映射：保留事件时间比例与阶梯，不重新求解占用、释放或可用性。 */
export function mixtureStudyGeometry(rolling:RollingProjection,capacity:number,plot:{left:number;top:number;width:number;height:number}) {
  const start=Date.parse(rolling.asOf),end=Date.parse(rolling.endsAt),span=end-start;
  const max=Math.max(capacity,...rolling.points.map(p=>usageTrendValue(p.usedPoints)??0),1);
  const x=(at:string)=>plot.left+(Date.parse(at)-start)/span*plot.width;
  const y=(v:number)=>plot.top+(1-v/max)*plot.height;
  const paths:string[]=[];let current="",lastKnown=false;
  for (const p of rolling.points) {
    const used=usageTrendValue(p.usedPoints);
    if (used===null) { if (current) { current+=` H ${x(p.at)}`; paths.push(current); } current="";lastKnown=false;continue; }
    if (!lastKnown) current=`M ${x(p.at)} ${y(used)}`;
    else current+=` H ${x(p.at)} V ${y(used)}`;
    lastKnown=true;
  }
  if (current) paths.push(current);
  const unknown=rolling.points.flatMap((p,i)=>p.usedPoints===null&&i<rolling.points.length-1?[{left:x(p.at),width:x(rolling.points[i+1].at)-x(p.at)}]:[]);
  return {max,x,y,paths,unknown,capacityY:y(capacity),coordinates:rolling.points.map(p=>({point:p,x:x(p.at),y:p.usedPoints===null?null:y(p.usedPoints)}))};
}
