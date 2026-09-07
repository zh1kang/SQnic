CREATE TABLE auto_projects(repo TEXT PRIMARY KEY, enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)));
CREATE TABLE auto_sessions(id INTEGER PRIMARY KEY, repo TEXT NOT NULL REFERENCES auto_projects(repo), harness TEXT NOT NULL, native_id TEXT NOT NULL, task TEXT REFERENCES tasks(id), branch TEXT NOT NULL, excluded INTEGER NOT NULL DEFAULT 0, ended INTEGER NOT NULL DEFAULT 0, injected INTEGER NOT NULL DEFAULT 0, seen INTEGER NOT NULL, error TEXT, UNIQUE(repo,harness,native_id));
CREATE INDEX auto_sessions_repo ON auto_sessions(repo,branch,task);
CREATE TABLE auto_files(session INTEGER NOT NULL REFERENCES auto_sessions(id), path TEXT NOT NULL, stamp TEXT, error TEXT, pending INTEGER NOT NULL DEFAULT 0, checked INTEGER, PRIMARY KEY(session,path));
CREATE TABLE auto_leases(repo TEXT PRIMARY KEY REFERENCES auto_projects(repo), token TEXT NOT NULL, expires INTEGER NOT NULL);
PRAGMA user_version=4;

CREATE TABLE auto_adapters(repo TEXT NOT NULL REFERENCES auto_projects(repo), harness TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(repo,harness));
