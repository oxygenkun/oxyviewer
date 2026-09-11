import fs from 'node:fs';
const [reportPath, outputPath] = process.argv.slice(2);
const report = JSON.parse(fs.readFileSync(reportPath, 'utf8'));
const lines = fs.readFileSync(`${reportPath}.stderr.log`, 'utf8').split(/\r?\n/);
const events = lines.filter(line => line.startsWith('HIF_PROFILE ')).map(line => {
  const kind = line.split(' ')[1];
  return {kind, ...Object.fromEntries([...line.matchAll(/(\w+)=([\d.e+-]+)/g)].map(m=>[m[1],Number(m[2])])),
    thread: line.includes('oxy-Thumbnail-')?'thumbnail':line.includes('thread=None')?'maintenance':undefined};
});
function stats(values) {
  const sorted = values.filter(Number.isFinite).sort((a,b)=>a-b);
  return {count:sorted.length,min:sorted[0],median:sorted[Math.floor(sorted.length/2)],p95:sorted[Math.min(sorted.length-1,Math.floor(sorted.length*.95))],max:sorted.at(-1),sum:sorted.reduce((a,b)=>a+b,0)};
}
const summary = {};
for (const kind of ['construct','prune','generation','admission','snapshot']) {
  const entries = events.filter(e=>e.kind===kind);
  summary[kind] = Object.fromEntries(['total','cleanup','wait','scan','held','revision','transition'].map(key=>[key,stats(entries.map(e=>e[key]))]).filter(([,value])=>value.count));
  if(kind==='construct') for(const thread of ['thumbnail','maintenance']) summary[kind][thread]=stats(entries.filter(e=>e.thread===thread).map(e=>e.total));
}
const end=report.marks.find(m=>m.name==='resource:stress-complete')?.detail;
summary.retained=end?.retained;
summary.queueRttMs=stats(end?.snapshotsMs??[]);
summary.jsBeatIntervalMs=stats(end?.beatsMs??[]);
const previews=report.marks.filter(m=>m.name==='preview:result'&&m.detail.level==='thumbnail');
summary.thumbnailResultCount=previews.length;
summary.first20NativeTotalMs=stats(previews.slice(0,20).map(m=>m.detail.diagnostics?.totalMs));
summary.last20NativeTotalMs=stats(previews.slice(-20).map(m=>m.detail.diagnostics?.totalMs));
summary.last20Construct=stats(events.filter(e=>e.kind==='construct'&&e.thread==='thumbnail').slice(-20).map(e=>e.total));
summary.last20Prune=stats(events.filter(e=>e.kind==='prune').slice(-20).map(e=>e.total));
summary.last20GenerationWait=stats(events.filter(e=>e.kind==='generation').slice(-20).map(e=>e.wait));
const output={summary,events,queueSamples:report.marks.filter(m=>m.name==='hif-profile:queue').map(m=>({t:m.t,...m.detail}))};
if(outputPath) fs.writeFileSync(outputPath,JSON.stringify(output,null,2)+'\n');
console.log(JSON.stringify(summary,null,2));
