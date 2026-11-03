/* Deterministic scheduler for the Unix WAL close/open regression.
 * Compile this standalone fixture with the exact bundled amalgamation.
 * The mutex wrappers only pause existing acquisitions; they add no SQLite locks.
 */
#include LOCAL_AGENT_SQLITE_SOURCE
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

static sqlite3_mutex_methods original;
static sqlite3_mutex *inode_mutex;
static pthread_mutex_t schedule = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static int close_entered, open_waiting;
static _Thread_local int role, big_depth;
static sqlite3 *closing;
static const char *database;

static void enter(sqlite3_mutex *mutex) {
  if (role == 2 && mutex == inode_mutex && big_depth) {
    pthread_mutex_lock(&schedule);
    open_waiting = 1;
    pthread_cond_broadcast(&changed);
    pthread_mutex_unlock(&schedule);
  }
  original.xMutexEnter(mutex);
  if (mutex == unixBigLock) big_depth++;
  if (role == 1 && mutex == inode_mutex && !big_depth && !close_entered) {
    pthread_mutex_lock(&schedule);
    close_entered = 1;
    pthread_cond_broadcast(&changed);
    while (!open_waiting) pthread_cond_wait(&changed, &schedule);
    pthread_mutex_unlock(&schedule);
  }
}
static void leave(sqlite3_mutex *mutex) {
  if (mutex == unixBigLock) big_depth--;
  original.xMutexLeave(mutex);
}
static void check(int rc, const char *step) {
  if (rc != SQLITE_OK) {
    fprintf(stderr, "%s: SQLite %d\n", step, rc);
    exit(2);
  }
}
static void *close_database(void *unused) {
  (void)unused;
  role = 1;
  check(sqlite3_close(closing), "close");
  return NULL;
}
static void *open_database(void *unused) {
  (void)unused;
  pthread_mutex_lock(&schedule);
  while (!close_entered) pthread_cond_wait(&changed, &schedule);
  pthread_mutex_unlock(&schedule);
  role = 2;
  sqlite3 *opened = NULL;
  check(sqlite3_open(database, &opened), "reopen");
  role = 0;
  check(sqlite3_busy_timeout(opened, 1000), "busy timeout");
  sqlite3_stmt *statement = NULL;
  check(sqlite3_prepare_v2(opened, "SELECT value FROM records", -1, &statement, NULL), "read");
  if (sqlite3_step(statement) != SQLITE_ROW || sqlite3_column_int(statement, 0) != 42) exit(3);
  check(sqlite3_finalize(statement), "finalize");
  check(sqlite3_close(opened), "final close");
  return NULL;
}
int main(int argc, char **argv) {
  if (argc != 2) return 2;
  database = argv[1];
  alarm(10);
  check(sqlite3_initialize(), "initialize default mutex methods");
  check(sqlite3_shutdown(), "shutdown before mutex configuration");
  check(sqlite3_config(SQLITE_CONFIG_GETMUTEX, &original), "get mutex methods");
  sqlite3_mutex_methods wrapped = original;
  wrapped.xMutexEnter = enter;
  wrapped.xMutexLeave = leave;
  check(sqlite3_config(SQLITE_CONFIG_MUTEX, &wrapped), "set mutex methods");
  check(sqlite3_open(database, &closing), "open");
  check(sqlite3_exec(closing, "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; CREATE TABLE records(value); INSERT INTO records VALUES(42);", NULL, NULL, NULL), "seed");
  sqlite3_file *file = NULL;
  check(sqlite3_file_control(closing, "main", SQLITE_FCNTL_FILE_POINTER, &file), "file pointer");
  inode_mutex = ((unixFile *)file)->pInode->pLockMutex;
  pthread_t closer, opener;
  if (pthread_create(&closer, NULL, close_database, NULL) || pthread_create(&opener, NULL, open_database, NULL)) return 2;
  pthread_join(closer, NULL);
  pthread_join(opener, NULL);
  puts(sqlite3_libversion());
  return 0;
}
