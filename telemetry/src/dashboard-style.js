// Colors mirror the editor families offered by Alter Zero's /theme picker.
// All surfaces and charts use roles so a theme changes the entire workspace.
export const DASHBOARD_STYLE = `
:root{color-scheme:dark;--bg:#11111b;--sidebar:#181825;--surface:#1e1e2e;--surface-2:#313244;--text:#cdd6f4;--muted:#a6adc8;--dim:#9399b2;--border:#353548;--accent:#89dceb;--purple:#cba6f7;--blue:#89b4fa;--good:#a6e3a1;--peach:#fab387;--red:#f38ba8;--on-accent:#11111b;--radius:14px;--mono:ui-monospace,SFMono-Regular,Consolas,"Liberation Mono",monospace}

:root[data-theme="macchiato"]{--bg:#181926;--sidebar:#1e2030;--surface:#24273a;--surface-2:#363a4f;--text:#cad3f5;--muted:#a5adcb;--dim:#939ab7;--border:#41465e;--accent:#91d7e3;--purple:#c6a0f6;--blue:#8aadf4;--good:#a6da95;--peach:#f5a97f;--red:#ed8796}

:root[data-theme="frappe"]{--bg:#232634;--sidebar:#292c3c;--surface:#303446;--surface-2:#414559;--text:#c6d0f5;--muted:#a5adce;--dim:#949cbb;--border:#51566f;--accent:#99d1db;--purple:#ca9ee6;--blue:#8caaee;--good:#a6d189;--peach:#ef9f76;--red:#e78284}

:root[data-theme="nord"]{--bg:#242933;--sidebar:#2e3440;--surface:#303744;--surface-2:#3b4252;--text:#eceff4;--muted:#c3cbd8;--dim:#acb8cb;--border:#48556a;--accent:#88c0d0;--purple:#b48ead;--blue:#81a1c1;--good:#a3be8c;--peach:#ebcb8b;--red:#bf616a}

:root[data-theme="dracula"]{--bg:#1e1f29;--sidebar:#232430;--surface:#282a36;--surface-2:#3a3c4e;--text:#f8f8f2;--muted:#c6c5d6;--dim:#adaac4;--border:#45475e;--accent:#8be9fd;--purple:#bd93f9;--blue:#a4baff;--good:#50fa7b;--peach:#ffb86c;--red:#ff79c6}

:root[data-theme="latte"]{color-scheme:light;--bg:#eff1f5;--sidebar:#e6e9ef;--surface:#ffffff;--surface-2:#e6e9ef;--text:#4c4f69;--muted:#5c5f77;--dim:#64677f;--border:#d1d5e1;--accent:#087d9f;--purple:#8839ef;--blue:#1e66f5;--good:#387e25;--peach:#b75a16;--red:#c32547;--on-accent:#fff}

@media(prefers-color-scheme:light){:root[data-theme="system"]{color-scheme:light;--bg:#eff1f5;--sidebar:#e6e9ef;--surface:#ffffff;--surface-2:#e6e9ef;--text:#4c4f69;--muted:#5c5f77;--dim:#64677f;--border:#d1d5e1;--accent:#087d9f;--purple:#8839ef;--blue:#1e66f5;--good:#387e25;--peach:#b75a16;--red:#c32547;--on-accent:#fff}
}

*{box-sizing:border-box}
html{scroll-behavior:smooth;scroll-padding-top:24px}
body{margin:0;background:var(--bg);color:var(--text);font:16px/1.5 ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;-webkit-font-smoothing:antialiased}
button,select{font:inherit}
button,a,select{-webkit-tap-highlight-color:transparent}
button,select{color:var(--text)}
button{cursor:pointer}
a{color:inherit;text-decoration:none}
button:disabled{cursor:default;opacity:.4}
button:focus-visible,a:focus-visible,select:focus-visible,summary:focus-visible,[tabindex]:focus-visible{outline:2px solid var(--accent);outline-offset:4px}
button,a,select,summary{touch-action:manipulation}
::selection{background:var(--accent);color:var(--on-accent)}
[hidden]{display:none!important}
.enhanced{display:none!important}
.js .enhanced{display:inline-flex!important}
.icon{width:20px;height:20px;flex:none;vertical-align:middle}
.sr-only{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0}
.skip-link{position:fixed;top:-100px;left:20px;z-index:20;background:var(--accent);color:var(--on-accent);padding:12px 20px;border-radius:6px}
.skip-link:focus{top:12px}
.mono{font-family:var(--mono)}
.muted{color:var(--muted)}

.sidebar{width:220px;position:fixed;inset:0 auto 0 0;background:var(--sidebar);border-right:1px solid var(--border);display:flex;flex-direction:column;padding:30px 18px;z-index:5}
.brand{display:flex;align-items:center;gap:12px;padding:0 10px;font-size:19px;font-weight:650;letter-spacing:-.5px}
.brand-mark{display:grid;place-items:center;width:38px;height:38px;background:color-mix(in srgb,var(--accent) 10%,transparent);border:1px solid color-mix(in srgb,var(--accent) 35%,transparent);color:var(--accent);border-radius:10px}
.brand-mark .icon{width:24px;height:24px}
.brand-sub{display:block;font:10px/1.8 var(--mono);letter-spacing:2.4px;color:var(--muted);margin-top:1px}
.sidebar-section{font:11px var(--mono);color:var(--dim);letter-spacing:1.6px;margin:48px 14px 16px}
.main-nav{display:grid;gap:7px}
.main-nav a{min-height:44px;display:flex;align-items:center;gap:12px;padding:10px 14px;border-radius:7px;color:var(--muted);font-size:14px;transition:background .15s,color .15s}
.main-nav a .icon{width:18px;height:18px}
.main-nav a:hover{background:var(--surface-2);color:var(--text)}
.main-nav .active{color:var(--accent);background:color-mix(in srgb,var(--accent) 9%,transparent)}
.main-nav a:not(.active) .nav-dot{visibility:hidden}
.nav-dot{height:5px;width:5px;border-radius:50%;background:var(--accent);margin-left:auto}
.sidebar-bottom{margin-top:auto;padding:28px 5px 0}
.privacy-note{display:flex;gap:9px;padding:15px 0;border-bottom:1px solid var(--border)}
.privacy-note>.icon{color:var(--accent);width:17px;height:17px;margin-top:3px}
.privacy-note strong{font-size:12px;font-weight:550}
.privacy-note p{font-size:12px;color:var(--muted);margin:5px 0 0;line-height:1.7}
.sidebar-signature{display:flex;align-items:center;gap:6px;font-size:10px;color:var(--dim);margin-top:20px}
.sidebar-signature .mono{font-size:16px;color:var(--accent);margin-left:auto}
.status-dot{width:6px;height:6px;background:var(--good);border-radius:100%;display:inline-block;flex:none}
.workspace{margin-left:220px}
.topbar{height:70px;padding:0 36px;border-bottom:1px solid var(--border);display:flex;align-items:center;justify-content:space-between;gap:15px}
.breadcrumb{display:flex;align-items:center;gap:12px;font-size:13px;color:var(--dim)}
.breadcrumb .icon{height:12px;width:12px}
.breadcrumb>span{color:var(--text)}
.topbar-tools{display:flex;gap:24px;align-items:center;font-size:12px}
.snapshot-label{display:flex;align-items:center;gap:8px;color:var(--muted)}
.json-link{display:flex;align-items:center;gap:7px;font:12px var(--mono);color:var(--muted)}
.json-link .icon{width:17px;height:17px}
.json-link:hover{color:var(--accent)}
main{max-width:1600px;padding:34px 36px 26px;margin:auto}
.page-heading{display:flex;align-items:center;justify-content:space-between;gap:20px}
.eyebrow{display:flex;align-items:center;gap:7px;font:10px/1.5 var(--mono);letter-spacing:1.8px;color:var(--dim)}
.eyebrow>span{height:5px;width:5px;background:var(--accent)}
h1{margin:6px 0 4px;font-size:32px;line-height:1.3;font-weight:620;letter-spacing:-1.1px}
.heading-dot{color:var(--accent)}
.page-heading p{font-size:14px;color:var(--muted);margin:0}
.heading-controls{display:flex;align-items:center;gap:9px}
.theme-control{align-items:center;gap:7px;background:var(--surface);border:1px solid var(--border);border-radius:7px;padding:0 10px;height:39px}
.theme-control>.icon{width:15px;height:15px;color:var(--purple)}
.theme-control select{border:0;background:var(--surface);font-size:12px;max-width:190px;height:36px;cursor:pointer}
.icon-button{height:39px;width:39px;align-items:center;justify-content:center;background:var(--surface);border:1px solid var(--border);border-radius:7px}
.icon-button .icon{width:16px;height:16px}
.icon-button:hover{color:var(--accent);border-color:var(--accent)}
.window-toolbar{display:flex;align-items:center;justify-content:space-between;margin:28px 0 20px;gap:16px}
.date-range{display:flex;align-items:center;gap:8px;font:12px var(--mono);color:var(--muted)}
.date-range>.icon{height:15px;width:15px}
.utc-badge{font:10px var(--mono);padding:2px 5px;border:1px solid var(--border);border-radius:4px;color:var(--dim);margin-left:3px}
.window-picker{display:flex;background:var(--sidebar);border:1px solid var(--border);padding:4px;border-radius:8px;gap:3px;flex-wrap:wrap}
.window-picker a{font:12px var(--mono);border-radius:5px;padding:5px 13px;color:var(--muted)}
.window-picker a:hover{color:var(--text);background:var(--surface-2)}
.window-picker a[aria-current]{background:var(--surface-2);color:var(--accent);box-shadow:0 1px 3px #0001}

.kpis{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:16px;margin-bottom:24px}
.kpi{padding:20px 21px;background:var(--surface);border:1px solid var(--border);border-radius:var(--radius);min-width:0;position:relative;overflow:hidden}
.kpi-highlight{border-color:color-mix(in srgb,var(--accent) 34%,var(--border));background:linear-gradient(115deg,color-mix(in srgb,var(--accent) 5%,var(--surface)),var(--surface))}
.kpi-highlight::before{content:"";position:absolute;top:0;left:22px;width:34px;height:2px;background:var(--accent)}
.kpi-top{display:flex;align-items:center;justify-content:space-between;gap:8px;font-size:13px;color:var(--muted)}
.kpi-top .icon{color:var(--dim);width:17px;height:17px}
.kpi-highlight .kpi-top .icon{color:var(--accent)}
.kpi>b{display:block;font:500 34px/1.2 var(--mono);letter-spacing:-1.2px;margin:17px 0 12px;overflow-wrap:anywhere}
.kpi-note{display:flex;align-items:center;gap:7px;font-size:11px;color:var(--dim);min-height:20px;flex-wrap:wrap}
.trend{font:11px var(--mono);background:var(--surface-2);border-radius:4px;padding:3px 5px}
.trend.positive{background:color-mix(in srgb,var(--good) 10%,transparent);color:var(--good)}
.trend.negative{background:color-mix(in srgb,var(--red) 10%,transparent);color:var(--red)}

.card{background:var(--surface);border:1px solid var(--border);border-radius:var(--radius);min-width:0;overflow:hidden}
.card-heading{display:flex;align-items:flex-start;justify-content:space-between;gap:15px;padding:22px 24px 0}
.card-heading h2{font-size:15px;font-weight:600;margin:0;letter-spacing:-.15px}
.card-heading p{margin:4px 0 0;font-size:12px;color:var(--dim)}
.activity-layout{display:grid;grid-template-columns:minmax(0,2.25fr) minmax(235px,1fr);gap:20px;margin-bottom:24px}
.chart-views{border:1px solid var(--border);padding:3px;gap:2px;border-radius:6px}
.chart-views button{border:0;background:transparent;color:var(--muted);font-size:11px;padding:3px 9px;border-radius:3px}
.chart-views button[aria-pressed="true"]{background:var(--surface-2);color:var(--text)}
.series-switch{margin:14px 24px 0;gap:18px}
.series-switch button{display:flex;align-items:center;gap:6px;padding:4px 0;background:none;border:0;color:var(--dim);font-size:12px}
.series-switch button[aria-pressed="true"]{color:var(--text)}
.series-switch i,.legend-item i{width:6px;height:6px;background:var(--accent);border-radius:2px}
.series-switch [data-series="new"] i{background:var(--purple)}
#activity-chart{padding:14px 24px 0}
.chart-meta{display:flex;align-items:center;justify-content:space-between;gap:12px;min-height:24px;color:var(--muted);font-size:11px;margin-bottom:15px}
.legend-item{display:flex;align-items:center;gap:6px}
#chart-readout{font:11px var(--mono);text-align:right}
.chart-plot{position:relative;height:175px;display:flex;gap:14px}
.chart-scale{display:flex;flex-direction:column;justify-content:space-between;font:10px var(--mono);color:var(--dim);text-align:right;width:28px;flex:none}
.chart-scale span{line-height:0}
.plot-inner{position:relative;flex:1;min-width:0}
.chart-grid{position:absolute;inset:0;display:flex;flex-direction:column;justify-content:space-between;pointer-events:none}
.chart-grid span{border-top:1px dashed var(--border);width:100%}
.bars{position:absolute;inset:0;list-style:none;padding:0;margin:0;display:flex;gap:min(5px,calc(30% / var(--day-count,30)))}
.bar-slot{flex:1;position:relative;min-width:0;isolation:isolate}
.bar-users,.bar-new{position:absolute;bottom:0;left:0;right:0;border-radius:3px 3px 0 0;transition:opacity .15s}
.bar-users{height:var(--h);background:var(--accent);opacity:.8}
.bar-new{height:var(--n);background:var(--purple);display:none}
.chart-hit{position:absolute;inset:-5px 0 0;border:0;background:none;border-radius:3px;display:none;width:100%;padding:0}
.js .chart-hit{display:block}
.bar-slot:hover .bar-users,.bar-slot:focus-within .bar-users{opacity:1}
.bar-slot:focus-within,.bar-slot.is-active{background:color-mix(in srgb,var(--accent) 7%,transparent)}
.chart-hit:focus-visible{outline:1px dashed var(--accent);outline-offset:2px}
.line-chart{display:none;position:absolute;inset:0;overflow:visible;height:100%;width:100%;pointer-events:none}
.line-chart polyline{fill:none;stroke:var(--accent);stroke-width:2;vector-effect:non-scaling-stroke}
.line-chart circle{fill:var(--accent)}
.line-area{fill:var(--accent);opacity:.08}
.line-new{display:none}
.line-new polyline{stroke:var(--purple)}
.line-new .line-area,.line-new circle{fill:var(--purple)}
[data-view="line"] .line-chart{display:block}
[data-view="line"] .bar-users,[data-view="line"] .bar-new{visibility:hidden}
[data-metric="new"] .bar-users{display:none}
[data-metric="new"] .bar-new{display:block}
[data-metric="new"] .line-users{display:none}
[data-metric="new"] .line-new{display:block}
[data-metric="new"] .legend-item i{background:var(--purple)}
.chart-axis{display:flex;justify-content:space-between;margin:13px 0 0 42px;font:10px var(--mono);color:var(--dim)}
.chart-bottom{display:flex;align-items:center;justify-content:space-between;gap:12px;border-top:1px solid var(--border);margin-top:21px;padding:13px 0;color:var(--dim);font-size:11px}
.chart-bottom a{color:var(--accent);white-space:nowrap}
.chart-bottom .icon{width:12px;height:12px}
.chart-empty{min-height:252px;display:flex;align-items:center;justify-content:center;flex-direction:column;text-align:center;gap:9px;padding:30px;color:var(--muted)}
.chart-empty>.icon{height:32px;width:32px;color:var(--accent);margin-bottom:8px}
.chart-empty strong{font-size:14px;font-weight:500}
.chart-empty span{font-size:12px}
.os-content{padding:22px 24px 0;display:flex;align-items:center;flex-direction:column;gap:21px}
.os-ring{width:138px;height:138px;background:var(--segments);border-radius:50%;padding:11px;transform:rotate(-90deg)}
.os-ring>div{width:100%;height:100%;display:flex;align-items:center;justify-content:center;flex-direction:column;background:var(--surface);border-radius:50%;transform:rotate(90deg)}
.os-ring .icon{width:18px;height:18px;color:var(--dim);margin-bottom:4px}
.os-ring b{font:24px/1.2 var(--mono)}
.os-ring span{font-size:11px;color:var(--dim)}
.os-list{width:100%;max-height:130px;overflow:auto}
.os-row{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:5px 0;font-size:12px}
.os-row>span{display:flex;align-items:center;gap:8px;overflow-wrap:anywhere}
.os-row i{height:6px;width:6px;border-radius:2px;flex:none}
.os-row b{font:12px var(--mono)}
.card-note{font-size:11px;color:var(--dim);line-height:1.65;margin:12px 24px 18px}
.os-card .card-note{margin-top:15px}

.geography-card{margin-bottom:24px}
.count-badge{display:flex;align-items:center;gap:6px;font:11px var(--mono);color:var(--muted);padding:5px 9px;border:1px solid var(--border);border-radius:5px;white-space:nowrap}
.count-badge .icon{width:13px;height:13px}
.geography-layout{display:grid;grid-template-columns:minmax(0,1.9fr) minmax(290px,1fr);margin-top:22px}
.map-panel{position:relative;min-width:0;padding:0 24px 18px}
.map-toolbar{display:flex;justify-content:space-between;align-items:center;gap:16px}
.map-legend{font-size:11px;color:var(--muted);display:flex;align-items:center;gap:7px}
.map-legend i{height:7px;width:7px;border-radius:50%;background:var(--accent);box-shadow:0 0 0 3px color-mix(in srgb,var(--accent) 12%,transparent)}
.map-controls{align-items:center;border:1px solid var(--border);border-radius:6px;background:var(--surface)}
.map-controls button{border:0;background:transparent;min-width:30px;min-height:30px;font:15px var(--mono)}
.map-controls button:hover{background:var(--surface-2)}
#map-zoom-reset{font-size:11px;border-left:1px solid var(--border);border-right:1px solid var(--border)}
.world-map-wrap{position:relative;margin-top:13px}
.world-map{display:block;width:100%;height:auto;min-height:190px;max-height:330px;overflow:hidden}
.world-map .map-marker{cursor:pointer;outline:none}
.world-map .map-marker:hover,.world-map .map-marker:focus-visible{filter:drop-shadow(0 0 4px var(--accent))}
.world-map .map-marker.is-selected{filter:drop-shadow(0 0 5px var(--accent))}
.map-marker.is-selected circle{stroke-width:2.5}
.map-status{display:none}
.map-footnote{display:flex;flex-wrap:wrap;column-gap:12px;row-gap:3px;font-size:10px!important;color:var(--dim);line-height:1.6;margin:0 0 14px}
.country-inspector{display:flex;align-items:center;gap:10px;border:1px solid var(--border);background:color-mix(in srgb,var(--surface-2) 35%,transparent);border-radius:8px;padding:10px 12px;min-height:45px;font-size:12px;color:var(--muted)}
.inspector-icon{color:var(--accent)}
.inspector-icon .icon{width:16px;height:16px}
.country-inspector output{flex:1;overflow-wrap:anywhere}
.text-button{padding:4px;background:transparent;border:0;font-size:11px;color:var(--accent)}
.country-panel{border-left:1px solid var(--border);padding:0 24px 10px;min-width:0}
.country-panel h3{font-size:13px;margin:0 0 12px;font-weight:550;display:flex;justify-content:space-between;align-items:center}
.country-panel h3 span{font:10px var(--mono);color:var(--dim)}
.country-panel .card-note{margin:16px 0 0}
.country-select{display:flex;align-items:center;gap:8px;color:var(--text);background:none;border:0;padding:4px 0;font-size:12px;text-align:left;width:100%;min-height:31px}
.country-name{overflow-wrap:anywhere}
.country-select .code{margin-left:auto}
.country-select:hover,.country-select.is-selected{color:var(--accent)}
.country-select.is-selected .country-name{text-decoration:underline;text-underline-offset:4px}
.flag{font-size:16px;flex:none}
.environment-layout{display:grid;grid-template-columns:1fr 1fr;gap:20px;margin-bottom:24px}
.environment-layout .card{padding-bottom:14px}
.environment-layout table{width:calc(100% - 48px);margin:17px 24px 0}
.environment-layout .more-rows{margin:0 24px}
.environment-layout .more-rows table{width:100%;margin:0}
.section-icon{display:grid;place-items:center;border:1px solid var(--border);height:30px;width:30px;border-radius:7px;color:var(--dim)}
.section-icon .icon{width:16px;height:16px}
table{border-collapse:collapse;width:100%;table-layout:auto;font-size:12px}
th,td{text-align:left;padding:11px 0;border-bottom:1px solid var(--border);font-weight:400;font-variant-numeric:tabular-nums}
thead th{color:var(--dim);font-size:10px;font-weight:400;text-transform:uppercase;letter-spacing:.7px;padding:9px 0}
tbody tr:last-child th,tbody tr:last-child td{border-bottom:0}
td.n,th.n{text-align:right;font:11px var(--mono);padding-left:12px;white-space:nowrap}
thead th.n{font-size:10px}
td.share,th.share{width:22%;padding-left:16px}
.meter{display:block;height:4px;background:var(--surface-2);border-radius:2px;overflow:hidden}
.meter>span{display:block;height:100%;background:var(--accent);border-radius:2px}
.environment-layout>.card:last-child .meter>span{background:var(--purple)}
.name{display:flex;align-items:center;gap:8px;min-width:0}
.txt{overflow-wrap:anywhere}
.code{font:10px var(--mono);color:var(--dim);overflow-wrap:anywhere}
.empty-cell{color:var(--dim);padding:22px 0;font-size:12px}
.more-rows summary{list-style:none;display:flex;align-items:center;justify-content:space-between;color:var(--accent);font-size:11px;cursor:pointer;padding:10px 0;border-top:1px solid var(--border)}
summary::-webkit-details-marker{display:none}
.more-rows summary .icon{height:12px;width:12px}
.more-rows[open] summary .icon{transform:rotate(90deg)}
.daily-data{margin-bottom:26px}
.daily-data>summary{list-style:none;cursor:pointer;display:flex;align-items:center;justify-content:space-between;gap:16px;padding:20px 24px}
.daily-title{display:flex;align-items:center;gap:12px;font-size:14px;font-weight:550}
.daily-title>.icon{color:var(--dim);width:18px;height:18px}
.daily-title>span>span{display:block;font-size:11px;font-weight:400;color:var(--dim);margin-top:3px}
.summary-end{display:flex;align-items:center;gap:9px;font-size:11px;color:var(--muted)}
.summary-end .icon{width:13px;height:13px}
.daily-data[open] .summary-end .icon{transform:rotate(90deg)}
.daily-data[open]>summary{border-bottom:1px solid var(--border)}
.daily-scroll{max-height:420px;overflow:auto;padding:0 24px 18px}
.daily-scroll thead{position:sticky;top:0;background:var(--surface)}
footer{color:var(--dim);font-size:11px}
.footer-top{display:flex;align-items:center;justify-content:space-between;gap:16px;font-size:10px}
.footer-top>span:first-child{display:flex;align-items:center;gap:8px;font-size:11px}
.footer-top .icon{color:var(--accent);height:16px;width:16px}
.footer-top strong{font-weight:550;color:var(--muted)}
.footer-divider{margin:0 4px}
footer p{max-width:790px;line-height:1.75;margin:13px 0 6px}
footer>a{color:var(--muted)}
footer>a:hover{color:var(--accent)}
footer>a>.icon{height:11px;width:11px}

@media(min-width:1600px){main{padding-top:42px}
.chart-plot{height:210px}
.world-map{max-height:400px}
.os-content{padding-top:32px;gap:30px}
.os-ring{height:155px;width:155px}
.kpi{padding:24px}
.kpi>b{font-size:40px}
}

@media(max-width:1200px){.sidebar{width:190px;padding:26px 12px}
.workspace{margin-left:190px}
.brand{font-size:17px;gap:9px}
.brand-mark{width:33px;height:33px}
.sidebar-signature{font-size:9px}
.topbar{padding:0 24px}
main{padding:28px 24px}
.kpis{gap:12px}
.kpi{padding:18px 16px}
.kpi-top{font-size:12px}
.kpi>b{font-size:30px}
.kpi-note{font-size:10px}
.geography-layout{grid-template-columns:minmax(0,1.5fr) minmax(270px,1fr)}
.card-heading{padding:20px 20px 0}
.map-panel{padding:0 20px 16px}
.country-panel{padding:0 20px 16px}
.chart-meta{flex-wrap:wrap;gap:4px}
.activity-layout{grid-template-columns:minmax(0,2fr) minmax(220px,1fr)}
.chart-bottom{font-size:10px}
.brand-sub{font-size:9px}
}

@media(max-width:1000px){.sidebar{width:76px;align-items:center;padding:24px 10px}
.brand{padding:0}
.brand>span:last-child,.sidebar-section,.main-nav a span,.sidebar-bottom{display:none}
.main-nav{margin-top:34px;width:100%}
.main-nav a{justify-content:center;padding:13px}
.workspace{margin-left:76px}
.geography-layout{grid-template-columns:minmax(0,1.4fr) minmax(260px,1fr)}
}

@media(max-width:800px){.activity-layout{grid-template-columns:1fr}
.os-card .os-content{flex-direction:row;padding:16px 24px;gap:30px}
.os-ring{width:112px;height:112px;flex:none;padding:9px}
.os-ring .icon{display:none}
.os-list{flex:1}
.os-card .card-note{margin-top:0}
.geography-layout{grid-template-columns:1fr}
.country-panel{border-left:0;border-top:1px solid var(--border);padding:20px 24px}
.map-panel{padding:0 24px 20px}
.world-map{max-height:none}
.kpis{grid-template-columns:repeat(2,minmax(0,1fr))}
.kpi>b{font-size:32px}
.kpi-note{font-size:11px}
.environment-layout{grid-template-columns:1fr}
.heading-controls{align-self:flex-start}
.theme-control select{max-width:142px}
.page-heading{gap:12px}
.page-heading p{font-size:13px}
.topbar-tools{gap:14px}
.snapshot-label{font-size:11px}
.footer-top{align-items:flex-start;flex-direction:column;gap:8px}
}

@media(max-width:560px){html{scroll-padding-top:12px}
.sidebar{position:static;width:auto;height:auto;display:block;padding:17px 18px 9px;border-right:0;border-bottom:1px solid var(--border)}
.brand{gap:10px;width:max-content}
.brand>span:last-child{display:block;font-size:17px}
.brand-sub{display:none}
.brand-mark{height:30px;width:30px}
.brand-mark .icon{width:20px;height:20px}
.main-nav{margin-top:15px;display:flex;justify-content:space-between;gap:5px;overflow:auto}
.main-nav a{padding:9px;min-height:36px;gap:6px}
.main-nav a span:not(.nav-dot){display:block;font-size:11px}
.main-nav a .icon{width:14px;height:14px}
.main-nav a:nth-child(4){display:none}
.workspace{margin-left:0}
.topbar{height:43px;padding:0 18px}
.breadcrumb{font-size:11px;gap:8px}
.topbar-tools{gap:12px}
.snapshot-label{font-size:10px}
.json-link{font-size:10px}
.json-link>.icon{display:none}
main{padding:23px 16px}
.page-heading{flex-wrap:wrap;gap:17px}
.page-heading h1{font-size:28px}
.eyebrow{font-size:9px}
.heading-controls{width:100%;justify-content:space-between}
.theme-control select{max-width:210px;font-size:12px}
.window-toolbar{flex-wrap:wrap;margin-top:22px;gap:14px}
.date-range{font-size:11px}
.window-picker{margin-left:auto}
.window-picker a{padding:5px 11px}
.kpis{gap:10px;margin-bottom:18px}
.kpi{padding:15px 14px}
.kpi-top{font-size:11px;align-items:flex-start}
.kpi-top .icon{width:14px;height:14px}
.kpi>b{font-size:29px;margin:14px 0 10px}
.kpi-note{font-size:10px;gap:5px}
.trend{font-size:10px}
.card-heading{padding:18px 18px 0}
.card-heading h2{font-size:14px}
.card-heading p{font-size:11px}
.activity-layout,.geography-card,.environment-layout{margin-bottom:18px;gap:18px}
.series-switch{margin-left:18px;margin-right:18px}
#activity-chart{padding:12px 18px 0}
.chart-meta{font-size:10px}
#chart-readout{font-size:10px}
.chart-plot{height:160px}
.chart-bottom{align-items:flex-start;font-size:10px}
.chart-bottom>span{max-width:160px}
.chart-axis{font-size:9px}
.chart-scale{font-size:9px}
.map-panel{padding:0 16px 16px}
.country-panel{padding:18px}
.count-badge{font-size:10px;padding:4px 6px}
.map-footnote{font-size:10px!important}
.country-inspector{font-size:11px}
.environment-layout table{width:calc(100% - 36px);margin:14px 18px 0}
.environment-layout .more-rows{margin:0 18px}
.daily-data>summary{padding:17px 18px}
.daily-title{font-size:13px}
.daily-title>span>span{font-size:10px}
.summary-end{font-size:10px}
.daily-scroll{padding:0 18px 15px}
.os-content{gap:24px}
.footer-top{font-size:10px}
.topbar-tools .snapshot-label{display:none}
}

/* Small metadata is supplementary; primary labels respect text enlargement. */
.brand-sub,.sidebar-section,.eyebrow{font-size:.75rem}
.sidebar-signature{font-size:.75rem}
.kpi-top,.series-switch button,.country-select,.os-row,table{font-size:.875rem}
.kpi-note,.card-heading p,.chart-meta,#chart-readout,.chart-axis,.chart-scale,.chart-bottom,.code,.card-note,.map-footnote,.country-inspector,.more-rows summary,.daily-title>span>span,footer,.footer-top,.date-range,.theme-control select,.window-picker a,.chart-views button,thead th,thead th.n,td.n,th.n{font-size:.75rem}
.card-heading h2,.page-heading p{font-size:1rem}
.map-footnote{font-size:.75rem!important}
.main-nav a{font-size:.875rem}
.theme-control{height:42px}
.window-picker a{padding-top:7px;padding-bottom:7px}
.chart-views button{min-height:28px}
.map-controls button{min-width:34px;min-height:34px}
.os-row b,.os-ring span{font-size:.75rem}
.country-panel h3{font-size:.875rem}
.country-panel h3 span,.summary-end,.count-badge{font-size:.75rem}

.world-map .map-country.is-selected{fill:var(--accent);fill-opacity:.3;stroke:var(--accent)}

.world-map .map-marker:focus-visible .map-marker-halo{fill-opacity:.4;stroke:var(--accent);stroke-width:1}

.js .enhanced[hidden]{display:none!important}

@media(prefers-reduced-motion:reduce){html{scroll-behavior:auto}
*,*::before,*::after{animation:none!important;transition:none!important}
}

`;
