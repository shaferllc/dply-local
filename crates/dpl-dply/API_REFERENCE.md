# dply v1 API reference (derived from dply-cli)

Response unwrapping convention: use `data` key if it's an array, else the top-level value.
`get(path, query)`, `delete(path, query)` → params in query string.
`post/put/patch(path, body)` → params in JSON body. Auth: `Authorization: Bearer <token>`.

## auth
- device flow: `POST /api/v1/auth/device/start` → {device_code,user_code,verification_uri,verification_uri_complete,expires_in,interval}
- `POST /api/v1/auth/device/poll` {device_code} → {status: pending|authorized|denied|expired, token?}
- post-login profile: `GET /api/v1/operator/summary`

## edge  (base /api/v1/edge/sites/{site})
- `GET /api/v1/edge/sites` q:status → rows id,name,status,build.framework|framework,live_url|hostname,updated_at
- `GET /api/v1/edge/sites/{site}` → id,name,status,edge_backend|backend,runtime_mode,build.*,source.*,live_url,active_deployment_id,created_at,updated_at
- `POST /api/v1/edge/sites/{site}/deployments` body:git_commit,git_branch (drop empty) → id,status
- `GET /api/v1/edge/sites/{site}/deployments` q:limit → rows id,status,git_commit,git_branch,published_at,created_at
- `GET /api/v1/edge/sites/{site}/deployments/{deployment}` → id,status,git_commit,git_branch,meta.*,storage_prefix,cf_kv_version,build_log_path,published_at,failed_at,failure_reason,created_at,aliases[]
- `POST /api/v1/edge/sites/{site}/deployments/{deployment}/rollback`
- access base /api/v1/edge/sites/{site}/access: PATCH body{mode,password,allowed_emails} (if any set); GET → mode,password_set|has_password,allowed_emails[],updated_at
- env base /api/v1/edge/sites/{site}/env: PUT body{vars:[{key,value,scope}],scope} (from-file); PATCH /env/{KEY} body{value,scope}; DELETE /env/{KEY} q{scope}; GET q{scope} → rows key,scope,updated_at
- domains base /api/v1/edge/sites/{site}/domains: POST body{hostname}; POST /domains/{hostname}/verify; DELETE /domains/{hostname}; GET → rows hostname,status,verified_at,created_at
- `GET /api/v1/edge/sites/{site}/aliases` → rows hostname,deployment_id|deployment.id,created_at
- previews base /api/v1/edge/sites/{site}/previews: POST body{branch}; DELETE /previews/{id}; POST /previews/{id}/promote; GET → rows id,preview_branch|branch,preview_pr_number,status,live_url,updated_at
- `GET /api/v1/edge/sites/{site}/usage` q:period → dump
- `POST /api/v1/edge/sites/{site}/cache/purge` body{} or {paths:[...]}
- `GET /api/v1/edge/sites/{site}/logs` q:limit,since (loop if --tail) → rows timestamp|logged_at,status,method,path|url,ms,message
- `POST /api/v1/edge/lint` body{path,content} → ok,errors[],warnings[]

## servers  (base /api/v1/servers)
- `GET /api/v1/servers` → rows id,name,provider,region,status,ip_address|ip,updated_at
- `POST /api/v1/servers/{server}/run-command` body{command,user} → stdout|output,stderr,exit_code
- firewall base /api/v1/servers/{server}/firewall: POST /templates/{template}; POST /bundled/{key}; POST /apply; GET → rules[]{action,protocol,port|port_range,source,comment}
- log-shipping base /api/v1/servers/{server}/log-shipping: POST /enable body{sources:{k:true}}; POST /resync; DELETE; GET → addon_enabled,installed,status,version,last_seen_at,sources{},destination,shipping,error_message

## sites  (base /api/v1/sites/{site})
- `GET /api/v1/sites` → rows id,name,server.name|server_name,runtime|runtime_profile,status,updated_at
- `GET /api/v1/sites/{site}` → name,slug,server_name,runtime,runtime_version,status,ssl_status,git_repository_url,git_branch,last_deploy_at
- `PATCH /api/v1/sites/{site}` body{name} → name,slug
- `POST /api/v1/sites/{site}/deploy` → id,status
- `GET /api/v1/sites/{site}/deployments` → rows id,status,commit|git_commit,started_at,finished_at
- `GET /api/v1/sites/{site}/deployments/{deployment}` → id,status,commit,branch,commit_author,commit_subject,started_at,finished_at,duration,log|output
- `GET /api/v1/sites/{site}/commits` → rows short_sha|sha,message,author_name,committed_at
- domains: POST /api/v1/sites/{site}/domains body{hostname,is_primary,www_redirect}; GET → rows hostname,is_primary,www_redirect; DELETE /domains/{hostname}
- basic-auth: POST /api/v1/sites/{site}/basic-auth body{username,password,path}; GET → rows username,path; DELETE /basic-auth/{username}
- `GET /api/v1/sites/{site}/databases` → rows name,engine,username,host,site_owned
- `GET /api/v1/sites/{site}/schedules` → deploy_schedules[],cron_jobs[]
- `GET /api/v1/sites/{site}/ssl` → ssl_status,data[]{provider_type,challenge_type,status,expires_at,last_installed_at}
- `GET /api/v1/sites/{site}/system-user` → username,server_name
- `GET /api/v1/sites/{site}/uptime` → rows label,path,status,http_status,latency_ms,last_checked_at
- `GET /api/v1/sites/{site}/workers` → rows type,name,command,scale,is_active
- `GET /api/v1/sites/{site}/errors` q:limit → rows category,title,occurred_at

## site (singular, VM env)  base /api/v1/sites/{site}/env
- PATCH /env/{KEY} body{value} (NO scope); DELETE /env/{KEY} (no query); GET → rows key

## insights / imports / operator
- `GET /api/v1/insights/summary` → dump
- `GET /api/v1/servers/{server}/insights` → findings[]|data[]{severity,category,title|message,detected_at|created_at,acknowledged_at}
- `GET /api/v1/imports/migrations` → rows id,source,status,item_count|items,updated_at
- `GET /api/v1/imports/migrations/{migration}` → scalar keys dump
- `GET /api/v1/operator/summary` → operator.*,organization.*
- `GET /api/v1/operator/readme` q — → markdown|body|content
