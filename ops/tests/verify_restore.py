"""Exercise the actual recovery table reader against the disposable test cluster."""
import importlib.util
import os
from pathlib import Path
import subprocess

spec = importlib.util.spec_from_file_location("restore", Path(__file__).parents[1] / "backup/restore-drill.py")
restore = importlib.util.module_from_spec(spec)
spec.loader.exec_module(restore)
pg = Path(os.environ["PG_BIN"])
env = os.environ.copy()
database = "restore_schema_regression"
restore.execute([str(pg / "createdb"), database], env=env)
try:
    restore.execute([str(pg / "psql"), "-X", "-v", "ON_ERROR_STOP=1", "--dbname", database, "-c",
                     "CREATE SCHEMA eunha; CREATE TABLE eunha.accounts(id bigint); INSERT INTO eunha.accounts VALUES(1); CREATE SCHEMA another; CREATE TABLE another.events(id bigint);"], env=env)
    assert restore.verify_database(pg, database, env) == 2
    # The public schema is empty: success proves application schemas were read.
    print("Recovery verification passed for two non-public application schemas.")
finally:
    restore.execute([str(pg / "dropdb"), database], env=env)
