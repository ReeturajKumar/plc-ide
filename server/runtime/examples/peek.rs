// Temporary: compare tagmast with what the IDE uses. Reads server/.env; prints no secrets.
use std::{env, fs};
fn main() {
    for line in fs::read_to_string(".env").unwrap_or_default().lines() {
        if let Some((k, v)) = line.split_once('=') { if env::var_os(k.trim()).is_none() { env::set_var(k.trim(), v.trim()); } }
    }
    let s = |k: &str| env::var(k).unwrap_or_default();
    let mut cfg = postgres::Config::new();
    cfg.host(&s("MYPLC_DB_HOST")).port(s("MYPLC_DB_PORT").parse().unwrap_or(5432)).dbname(&s("MYPLC_DB_NAME")).user(&s("MYPLC_DB_USER")).password(s("MYPLC_DB_PASSWORD"));
    let mut c = cfg.connect(postgres::NoTls).expect("connect");
    let q = |c: &mut postgres::Client, sql: &str| for r in c.query(sql, &[]).unwrap() { let v: Vec<String> = (0..r.len()).map(|i| r.get::<_, Option<String>>(i).unwrap_or("NULL".into())).collect(); println!("  {}", v.join(" | ")); };
    println!("tagmast (name | tag_id | data_type | in scada_master_tags? | enabled):");
    q(&mut c, "SELECT t.tag_name, t.tag_id, t.data_type, (s.tag_name IS NOT NULL)::text, s.is_enabled::text FROM tagmast t LEFT JOIN scada_master_tags s ON s.tag_name=t.tag_name ORDER BY 1 LIMIT 40");
    println!("count tagmast:"); q(&mut c, "SELECT count(*)::text FROM tagmast");
    println!("st_tags (name | tagid | in tagmast?):");
    q(&mut c, "SELECT st.tag_name, st.tagid::text, EXISTS(SELECT 1 FROM tagmast t WHERE t.tag_name=st.tag_name)::text FROM st_tags st ORDER BY 1 LIMIT 40");
    println!("st_tags columns:"); q(&mut c, "SELECT string_agg(column_name || ' ' || data_type || coalesce(' default ' || column_default, ''), '; ' ORDER BY ordinal_position) FROM information_schema.columns WHERE table_name='st_tags'"); println!("st_tags constraints/triggers:"); q(&mut c, "SELECT string_agg(conname || ' ' || contype, ', ') FROM pg_constraint WHERE conrelid='st_tags'::regclass"); q(&mut c, "SELECT string_agg(tgname || ' on ' || tgrelid::regclass::text, ', ') FROM pg_trigger WHERE NOT tgisinternal");
}
