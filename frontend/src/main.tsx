import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { Activity, CircleAlert, Database, LogOut, Moon, RefreshCw, Server, Sun } from "lucide-react";
import "./styles.css";

type Tenant = { id:string; domain:string; title:string; desired_state:string; observed_state:string; config_revision:number; rendered_revision:number|null; runtime_name:string; bind_address:string; updated_at:string };
type Operation = { id:string; domain:string|null; kind:string; status:string; error:string|null; created_at:string };

async function api<T>(path:string, init?:RequestInit):Promise<T>{
  const response=await fetch(`/api${path}`,{...init,headers:{"Content-Type":"application/json",...(init?.headers||{})}});
  if(!response.ok) throw new Error((await response.json().catch(()=>({}))).error||`Request failed (${response.status})`);
  return response.status===204 ? undefined as T : response.json();
}

function ThemeButton(){
  const [dark,setDark]=useState(()=>localStorage.theme==="dark"||(!localStorage.theme&&matchMedia("(prefers-color-scheme: dark)").matches));
  useEffect(()=>{document.documentElement.classList.toggle("dark",dark);localStorage.theme=dark?"dark":"light"},[dark]);
  return <button className="icon-button" onClick={()=>setDark(!dark)} aria-label="Switch color theme">{dark?<Sun/>:<Moon/>}</button>
}

function BrandMark({small=false}:{small?:boolean}){
  return <span className={`brand-mark${small?" small":""}`} aria-hidden="true"><img className="light-symbol" src="/eunha-symbol-light.svg" alt=""/><img className="dark-symbol" src="/eunha-symbol-dark.svg" alt=""/></span>
}

function Login({onLogin}:{onLogin:()=>void}){
  const [username,setUsername]=useState("admin"),[password,setPassword]=useState(""),[error,setError]=useState("");
  async function submit(e:React.FormEvent){e.preventDefault();setError("");try{await api("/session",{method:"POST",body:JSON.stringify({username,password})});onLogin()}catch(e){setError(String(e instanceof Error?e.message:e))}}
  return <main className="login-shell"><section className="login-card"><BrandMark/><p className="eyebrow">EUNHA.SPACE</p><h1>Control plane</h1><p className="muted">Sign in to operate hosted Eunha instances.</p><form onSubmit={submit}><label>Operator<input value={username} onChange={e=>setUsername(e.target.value)} autoComplete="username"/></label><label>Password<input type="password" value={password} onChange={e=>setPassword(e.target.value)} autoComplete="current-password"/></label>{error&&<p className="error">{error}</p>}<button className="primary" type="submit">Sign in</button></form></section><ThemeButton/></main>
}

function App(){
  const [user,setUser]=useState<string|null|undefined>(undefined),[tenants,setTenants]=useState<Tenant[]>([]),[ops,setOps]=useState<Operation[]>([]),[busy,setBusy]=useState<string|null>(null),[error,setError]=useState("");
  async function load(){try{const me=await api<{username:string}>("/session/me");setUser(me.username);const [t,o]=await Promise.all([api<Tenant[]>("/tenants"),api<Operation[]>("/operations")]);setTenants(t);setOps(o)}catch{setUser(null)}}
  useEffect(()=>{load()},[]);
  if(user===undefined)return <div className="loading">Loading control plane…</div>;
  if(!user)return <Login onLogin={load}/>;
  async function check(id:string){setBusy(id);setError("");try{await api(`/tenants/${id}/check`,{method:"POST"});await load()}catch(e){setError(String(e instanceof Error?e.message:e))}finally{setBusy(null)}}
  async function logout(){await api("/session",{method:"DELETE"});setUser(null)}
  return <div className="app-shell"><aside><div className="brand"><BrandMark small/><div><strong>eunha.space</strong><span>Control plane</span></div></div><nav><a className="active"><Server/>Instances</a><a><Activity/>Operations</a></nav><div className="aside-foot"><span>{user}</span><button className="icon-button" onClick={logout} aria-label="Sign out"><LogOut/></button></div></aside><main className="workspace"><header><div><p className="eyebrow">RUNTIME FLEET</p><h1>Instances</h1></div><ThemeButton/></header>{error&&<div className="notice"><CircleAlert/>{error}</div>}<section className="summary"><div><span>Managed</span><strong>{tenants.length}</strong></div><div><span>Healthy</span><strong>{tenants.filter(t=>t.observed_state==="healthy").length}</strong></div><div><span>Pending render</span><strong>{tenants.filter(t=>t.rendered_revision!==t.config_revision).length}</strong></div></section><section className="panel"><div className="panel-title"><div><h2>Tenant inventory</h2><p>Configuration values live in the control-plane database.</p></div></div>{tenants.length===0?<div className="empty">No instances have been adopted yet.</div>:<div className="tenant-list">{tenants.map(t=><article key={t.id}><div className={`status ${t.observed_state}`}/><div className="tenant-main"><h3>{t.domain}</h3><p>{t.title}</p></div><dl><div><dt>Runtime</dt><dd>{t.runtime_name}</dd></div><div><dt>Config</dt><dd>revision {t.config_revision}{t.rendered_revision!==t.config_revision&&<span className="pending"> · not rendered</span>}</dd></div><div><dt>State</dt><dd>{t.observed_state}</dd></div></dl><button className="secondary" disabled={busy===t.id} onClick={()=>check(t.id)}><RefreshCw className={busy===t.id?"spin":""}/>Check</button></article>)}</div>}</section><section className="panel operations"><div className="panel-title"><div><h2>Recent operations</h2><p>Every lifecycle action is retained for audit and recovery.</p></div></div>{ops.length===0?<div className="empty">No operations recorded.</div>:<table><thead><tr><th>Operation</th><th>Instance</th><th>Status</th><th>When</th></tr></thead><tbody>{ops.map(o=><tr key={o.id}><td>{o.kind.replaceAll("_"," ")}</td><td>{o.domain||"—"}</td><td><span className={`pill ${o.status}`}>{o.status}</span></td><td>{new Date(o.created_at).toLocaleString()}</td></tr>)}</tbody></table>}</section><footer><Database/>Control-plane PostgreSQL is authoritative · secret values never leave the API</footer></main></div>
}
createRoot(document.getElementById("root")!).render(<React.StrictMode><App/></React.StrictMode>);
