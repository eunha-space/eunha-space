import { useEffect, useState } from "react";
import { useLingui } from "@lingui/react";
import { api } from "@/api";
import { Card, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";

export type LimitReading = { tenant_id:string; domain:string; resource:"users"|"media"|"database"|"activities"; value:number|null; allowance:number|null; sampled_at:number|null; fresh:boolean; exempt:boolean; override_exempt:boolean; over_since:string|null; grace_until:string|null; enforced:boolean; mode:string };
export type OverLimit = Pick<LimitReading,"tenant_id"|"domain"|"resource"|"value"|"allowance"|"sampled_at">;
function resourceLabel(resource:LimitReading["resource"],ko:boolean) {
 return (ko?{users:"사용자",media:"로컬 미디어",database:"데이터베이스",activities:"이번 달 외부 전송 (UTC)"}:{users:"Users",media:"Local media",database:"Database",activities:"Outbound deliveries this month (UTC)"})[resource];
}
/** How close a reading is to its allowance; exempt and unmeasured readings count as safe. */
type UsageLevel = "safe" | "near" | "over";
const NEAR_LIMIT = 0.8;
function usageLevel(row: LimitReading): UsageLevel {
  if (row.exempt || row.value === null || row.allowance === null) return "safe";
  if (row.enforced || row.value >= row.allowance) return "over";
  return row.value >= row.allowance * NEAR_LIMIT ? "near" : "safe";
}
const LEVEL_RANK: Record<UsageLevel, number> = { safe: 0, near: 1, over: 2 };
const CARD_TONE: Record<UsageLevel, string> = {
  safe: "bg-success/10 ring-success/40",
  near: "bg-warning/10 ring-warning/50",
  over: "bg-destructive/10 ring-destructive/50",
};
const BAR_TONE: Record<UsageLevel, string> = {
  safe: "bg-success",
  near: "bg-warning",
  over: "bg-destructive",
};
export function usageAmount(value:number|null,resource:string) {
  if(value===null) return "—";
  return resource==="media" || resource==="database" ? `${(value/1e9).toLocaleString(undefined,{maximumFractionDigits:2})} GB` : value.toLocaleString();
}
export function OverLimitWarning({rows}:{rows?:OverLimit[]}) {
  const {i18n}=useLingui(); const ko=i18n.locale==="ko";
  if(!rows?.length) return null;
  return <div role="note" className="bg-muted grid gap-2 rounded-lg p-3 text-sm">
    <p>{ko?"일부 인스턴스가 새 요금제 허용량에 도달했거나 초과했습니다. 변경은 계속 진행할 수 있습니다. 적용 시 14일의 유예 기간이 시작되며 새 가입 및 미디어 업로드만 제한될 수 있습니다. 데이터베이스와 활동은 공정 이용 안내입니다.":"Some instances meet or exceed the new plan allowances. You can continue. A 14-day grace period starts when the plan applies; only new registrations and media uploads may then be restricted. Database size and activities remain advisory."}</p>
    <ul className="list-inside list-disc">{rows.map(row=><li key={`${row.tenant_id}-${row.resource}`}>{row.domain} · {resourceLabel(row.resource,ko)}: {usageAmount(row.value,row.resource)} / {usageAmount(row.allowance,row.resource)}</li>)}</ul>
  </div>;
}
export function MemberUsage({tenantId}:{tenantId?:string}) {
 const {i18n}=useLingui(); const ko=i18n.locale==="ko";
 const [rows,setRows]=useState<LimitReading[]>([]); const [error,setError]=useState(false); const [queued,setQueued]=useState<string|null>(null);
 useEffect(()=>{let disposed=false;let busy=false;async function load(){if(busy||document.hidden)return;busy=true;try{const result=await api<LimitReading[]>("/customer/usage");if(!disposed){setRows(result);setError(false);}}catch{if(!disposed)setError(true);}finally{busy=false;}}void load();const timer=setInterval(()=>void load(),30000);return()=>{disposed=true;clearInterval(timer);};},[]);
 const selected=rows.filter(row=>!tenantId||row.tenant_id===tenantId);const ids=[...new Set(selected.map(row=>row.tenant_id))];
 if(!ids.length&&!error) return null;
 const labels=ko?{users:"사용자",media:"로컬 미디어",database:"데이터베이스",activities:"이번 달 외부 전송 (UTC)"}:{users:"Users",media:"Local media",database:"Database",activities:"Outbound deliveries this month (UTC)"};
 return <div className="grid gap-3">{error&&<p role="status" className="text-destructive text-sm">{ko?"사용량을 불러오지 못했습니다. 이전 측정값은 오래되었을 수 있습니다.":"Usage could not be refreshed. Previous readings may be stale."}</p>}{ids.map(id=>{const readings=selected.filter(row=>row.tenant_id===id);const level=readings.map(usageLevel).reduce<UsageLevel>((worst,next)=>LEVEL_RANK[next]>LEVEL_RANK[worst]?next:worst,"safe");return <Card key={id} className={CARD_TONE[level]}><CardContent className="grid gap-4">
 <div className="flex flex-wrap items-center justify-between gap-2"><h3 className="font-semibold">{readings[0].domain} · {ko?"요금제 사용량":"Plan usage"}</h3><Button size="sm" variant="outline" disabled={queued===id} onClick={async()=>{try{await api(`/customer/tenants/${id}/usage/check`,{method:"POST"});setQueued(id);setTimeout(()=>setQueued(null),30000);}catch{setError(true);}}}>{queued===id?(ko?"측정 요청됨":"Check queued"):(ko?"지금 측정":"Check now")}</Button></div>
 <div className="grid gap-4 sm:grid-cols-2">{readings.map(row=><div key={row.resource} className="grid gap-1.5"><div className="flex justify-between gap-2 text-sm"><span>{labels[row.resource]}</span><span className="tabular-nums">{usageAmount(row.value,row.resource)}{row.allowance===null?"":` / ${usageAmount(row.allowance,row.resource)}`}</span></div>
 {row.allowance!==null&&row.value!==null&&<Progress aria-label={labels[row.resource]} indicatorClassName={BAR_TONE[usageLevel(row)]} value={Math.min(100,row.allowance===0?100:row.value/row.allowance*100)} />}
 <p className="text-muted-foreground text-xs">{row.sampled_at===null?(ko?"측정값 없음":"Not measured"):new Date(row.sampled_at).toLocaleString()}{row.value===null?` · ${ko?"측정 불가":"Unavailable"}`:!row.fresh&&row.sampled_at!==null?` · ${ko?"오래된 측정값":"Stale"}`:""}{row.exempt?` · ${ko?"제한 면제":"Exempt from limits"}`:""}{["database","activities"].includes(row.resource)?` · ${ko?"공정 이용":"Fair use"}`:""}</p>
 {row.enforced&&<p className="text-destructive text-xs">{row.resource==="users"?(ko?"새 가입이 제한되었습니다.":"New registrations are restricted."):(ko?"새 미디어 업로드가 제한되었습니다.":"New media uploads are restricted.")}</p>}
 {!row.enforced&&row.grace_until&&<p className="text-xs">{ko?"유예 종료":"Grace ends"}: {new Date(row.grace_until).toLocaleString()}</p>}
 </div>)}</div><p className="text-muted-foreground text-xs">{ko?"미디어는 기록된 원본과 썸네일 기준이며 원격 캐시는 제외됩니다. 전송량은 관측한 성공 전송의 하한값입니다. 측정 누락은 0으로 간주하지 않습니다.":"Media counts recorded originals and thumbnails; remote cache is excluded. Deliveries are observed successful sends and may undercount during collection gaps. Missing readings are never treated as zero."}</p>
 </CardContent></Card>;})}</div>;
}
