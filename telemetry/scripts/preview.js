// A local design preview, never a collector: no database, requests or writes.
import { createServer } from 'node:http';
import { renderDashboard, shapeStats, daysBefore, utcDay, windowDays } from '../src/lib.js';

function sampleStats(days, empty) {
  const today = utcDay();
  const daily = [], newInstalls = [];
  for (let back = days - 1; back >= 0 && !empty; back -= 1) {
    const day = daysBefore(today, back);
    const wave = Math.sin(back * 1.71) * 32 + Math.cos(back * .49) * 23;
    const users = Math.max(12, Math.round(214 - Math.min(back, 180) * .8 + wave));
    daily.push({ day, users });
    newInstalls.push({ day, installs: Math.max(1, Math.round(users * (.12 + (back % 5) * .015))) });
  }
  return shapeStats({
    today, days, daily, newInstalls,
    countries: empty ? [] : [['US',448],['DE',224],['PH',183],['GB',146],['IN',128],['CA',94],['JP',79],['BR',63],['FR',54],['AU',41],['NL',37],['SG',30],['SE',28],['KR',24],['ZA',18],['MX',12],['NZ',9],['ZZ',5]].map(([country, users]) => ({country, users})),
    versions: empty ? [] : [{version:'0.1.0',users:1398},{version:'0.1.0-dev',users:250}],
    oses: empty ? [] : [{os:'linux',users:1171},{os:'macos',users:412},{os:'windows',users:65}],
    platforms: empty ? [] : [{platform:'ubuntu',os_version:'24.04',users:526},{platform:'arch',os_version:'',users:385},{platform:'macos',os_version:'15.3.1',users:412},{platform:'fedora',os_version:'42',users:164},{platform:'debian',os_version:'12',users:96},{platform:'windows',os_version:'',users:65}],
    totals: empty ? {} : {users_7d:682,users_30d:1648,installs:2039},
    generatedAt: new Date().toISOString(),
  });
}

const port = Number(process.env.TELEMETRY_PREVIEW_PORT || 8788);
const server = createServer((request, response) => {
  const url = new URL(request.url, 'http://localhost');
  if (!['/', '/v1/stats'].includes(url.pathname)) {
    response.writeHead(404).end('Not found');
    return;
  }
  const stats = sampleStats(windowDays(url.searchParams.get('days')), url.searchParams.get('empty') === '1');
  const isJson = url.pathname === '/v1/stats';
  const previewLabel = '<div style="padding:10px 16px;margin-bottom:22px;border:1px dashed var(--accent);border-radius:8px;color:var(--accent);font-size:13px">Local preview · Sample data <a style="float:right;text-decoration:underline" href="/?empty=' + (url.searchParams.get('empty') === '1' ? '0' : '1') + '">Toggle empty state</a></div>';
  const body = isJson ? JSON.stringify(stats, null, 2) : renderDashboard(stats)
    .replace('<main id="main">', '<main id="main">' + previewLabel)
    .replace('<title>Telemetry', '<title>Sample preview · Telemetry');
  response.writeHead(200, {'content-type': isJson ? 'application/json' : 'text/html; charset=utf-8', 'cache-control':'no-store'}).end(body);
});
server.listen(port, '127.0.0.1', () => console.log(`Telemetry sample preview: http://127.0.0.1:${port}`));
