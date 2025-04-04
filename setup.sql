-- Create user if not exists
DO
$do$
BEGIN
   IF NOT EXISTS (
      SELECT FROM pg_catalog.pg_roles
      WHERE  rolname = 'boltz') THEN
      CREATE USER boltz WITH PASSWORD 'boltz';
   END IF;
END
$do$;

-- Create database if not exists
CREATE DATABASE covclaim;

-- Connect to the database
\c covclaim

-- Create schema if not exists
CREATE SCHEMA IF NOT EXISTS covclaim;

-- Set search path
SET search_path TO covclaim;

-- Grant schema privileges
GRANT ALL ON SCHEMA covclaim TO boltz;
GRANT ALL ON ALL TABLES IN SCHEMA covclaim TO boltz;
GRANT ALL ON ALL SEQUENCES IN SCHEMA covclaim TO boltz;
GRANT ALL ON ALL FUNCTIONS IN SCHEMA covclaim TO boltz;

-- Set default privileges for future objects
ALTER DEFAULT PRIVILEGES IN SCHEMA covclaim GRANT ALL ON TABLES TO boltz;
ALTER DEFAULT PRIVILEGES IN SCHEMA covclaim GRANT ALL ON SEQUENCES TO boltz;
ALTER DEFAULT PRIVILEGES IN SCHEMA covclaim GRANT ALL ON FUNCTIONS TO boltz; 