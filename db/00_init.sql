CREATE TABLE IF NOT EXISTS modules (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  location TEXT NOT NULL,
  user TEXT,
  reason TEXT NOT NULL,
  depends TEXT,
  date TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS files (
  id INTEGER PRIMARY KEY,
  module_id INTEGER,
  source TEXT,
  source_checksum TEXT,
  destination TEXT NOT NULL UNIQUE,
  destination_checksum TEXT,
  operation TEXT NOT NULL,
  user TEXT,
  date TEXT NOT NULL,
  FOREIGN KEY (module_id) REFERENCES modules(id)
  ON DELETE CASCADE ON UPDATE CASCADE
);

CREATE TABLE IF NOT EXISTS backups (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  file_type TEXT NOT NULL,
  content BLOB,
  link_source TEXT,
  owner TEXT NOT NULL,
  permissions INTEGER,
  checksum TEXT,
  date TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
  id INTEGER PRIMARY KEY,
  setting TEXT NOT NULL UNIQUE,
  value TEXT NOT NULL
);

INSERT OR IGNORE INTO settings(setting, value) VALUES('schema_version', '1')
