#define _GNU_SOURCE
#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <pwd.h>
#include <shadow.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>
#include <crypt.h>

#define PASSWD_FILE "/etc/passwd"
#define PASSWD_MAX_LINE 1024
#define USERNAME_MAX 32
#define PASSWORD_MAX 256
#define HOME_DIR_PREFIX "/home"

static void usage(const char *progname) {
    printf("Usage: %s [OPTION]...\n", progname);
    printf("Linux-style login daemon\n\n");
    printf("Options:\n");
    printf("  -h, --help              Show this help message\n");
    printf("  -u, --user USERNAME     Create new user\n");
    printf("  login                   Prompt for login\n");
}

static int read_line(int fd, char *buf, size_t bufsz) {
    if (!buf || bufsz == 0)
        return -1;
    
    size_t pos = 0;
    while (pos < bufsz - 1) {
        ssize_t n = read(fd, buf + pos, 1);
        if (n <= 0) {
            if (pos == 0)
                return -1;
            break;
        }
        if (buf[pos] == '\n') {
            buf[pos] = '\0';
            return (int)pos;
        }
        pos++;
    }
    buf[pos] = '\0';
    return (int)pos;
}

static char *get_password(const char *prompt) {
    static char password[PASSWORD_MAX];
    struct termios old_termios, new_termios;
    int tty_fd = open("/dev/tty", O_RDWR);
    int echo_was_disabled = 0;
    
    if (tty_fd < 0) {
        tty_fd = STDIN_FILENO;
    }
    
    printf("%s", prompt);
    fflush(stdout);
    
    // Disable echo
    if (tcgetattr(tty_fd, &old_termios) == 0) {
        new_termios = old_termios;
        new_termios.c_lflag &= ~(ECHO | ECHOE | ECHOK | ECHONL);
        if (tcsetattr(tty_fd, TCSAFLUSH, &new_termios) == 0) {
            echo_was_disabled = 1;
        }
    }
    
    ssize_t len = read_line(tty_fd, password, sizeof(password));
    
    // Restore echo (use saved old_termios, don't call tcgetattr again)
    if (echo_was_disabled) {
        tcsetattr(tty_fd, TCSAFLUSH, &old_termios);
    }
    
    if (tty_fd != STDIN_FILENO) {
        close(tty_fd);
    }
    
    if (len < 0) {
        return NULL;
    }
    
    return password;
}

static int validate_username(const char *username) {
    if (!username || username[0] == '\0')
        return 0;
    
    size_t len = strlen(username);
    if (len > USERNAME_MAX)
        return 0;
    
    // Username must start with letter or underscore
    if (!isalnum((unsigned char)username[0]) && username[0] != '_')
        return 0;
    
    // Rest must be alphanumeric, underscore, or dash
    for (size_t i = 1; i < len; i++) {
        if (!isalnum((unsigned char)username[i]) && 
            username[i] != '_' && username[i] != '-')
            return 0;
    }
    
    return 1;
}

static int user_exists(const char *username) {
    FILE *fp = fopen(PASSWD_FILE, "r");
    if (!fp)
        return 0;
    
    char line[PASSWD_MAX_LINE];
    while (fgets(line, sizeof(line), fp)) {
        // Remove newline
        size_t len = strlen(line);
        if (len > 0 && line[len - 1] == '\n')
            line[len - 1] = '\0';
        
        // Parse passwd entry: username:password:uid:gid:gecos:home:shell
        char *colon = strchr(line, ':');
        if (!colon)
            continue;
        
        size_t userlen = (size_t)(colon - line);
        if (userlen == strlen(username) && 
            strncmp(line, username, userlen) == 0) {
            fclose(fp);
            return 1;
        }
    }
    
    fclose(fp);
    return 0;
}

static uid_t get_next_uid(void) {
    uid_t max_uid = 1000; // Start from 1000 for regular users
    FILE *fp = fopen(PASSWD_FILE, "r");
    if (!fp)
        return max_uid;
    
    char line[PASSWD_MAX_LINE];
    while (fgets(line, sizeof(line), fp)) {
        // Parse passwd entry
        char *fields[7];
        int field_idx = 0;
        char *p = line;
        char *start = p;
        
        while (*p && field_idx < 7) {
            if (*p == ':') {
                *p = '\0';
                fields[field_idx++] = start;
                start = p + 1;
            }
            p++;
        }
        if (field_idx < 7)
            fields[field_idx++] = start;
        
        if (field_idx >= 3) {
            // fields[2] is UID
            uid_t uid = (uid_t)atoi(fields[2]);
            if (uid >= max_uid)
                max_uid = uid + 1;
        }
    }
    
    fclose(fp);
    return max_uid;
}

static int create_directory_recursive(const char *path) {
    // Check if directory already exists
    struct stat st;
    if (stat(path, &st) == 0) {
        if (S_ISDIR(st.st_mode)) {
            return 0; // Already exists
        } else {
            fprintf(stderr, "logind: '%s' exists but is not a directory\n", path);
            return -1;
        }
    }
    
    // Try to create the directory
    if (mkdir(path, 0755) == 0) {
        return 0; // Success
    }
    
    // If parent doesn't exist, try to create parent first
    char parent[512];
    const char *last_slash = strrchr(path, '/');
    if (last_slash && last_slash != path) {
        size_t parent_len = (size_t)(last_slash - path);
        if (parent_len >= sizeof(parent))
            return -1;
        strncpy(parent, path, parent_len);
        parent[parent_len] = '\0';
        
        // Recursively create parent
        if (create_directory_recursive(parent) != 0)
            return -1;
        
        // Try again to create the directory
        if (mkdir(path, 0755) == 0)
            return 0;
    }
    
    // Check if it was created by another process (race condition)
    if (stat(path, &st) == 0 && S_ISDIR(st.st_mode)) {
        return 0; // Created by another process, that's fine
    }
    
    return -1;
}

static int create_user(const char *username, const char *password) {
    if (!validate_username(username)) {
        fprintf(stderr, "logind: invalid username '%s'\n", username);
        return 1;
    }
    
    if (user_exists(username)) {
        fprintf(stderr, "logind: user '%s' already exists\n", username);
        return 1;
    }
    
    if (!password || strlen(password) == 0) {
        fprintf(stderr, "logind: password cannot be empty\n");
        return 1;
    }
    
    // Generate salt for MD5 crypt ($1$salt$)
    // Use a combination of time and random data for salt
    char salt_chars[] = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789./";
    char salt[16];
    time_t now = time(NULL);
    unsigned seed = (unsigned)(now ^ (unsigned long)password);
    
    // Simple PRNG for salt generation
    salt[0] = salt_chars[seed % (sizeof(salt_chars) - 1)];
    salt[1] = salt_chars[(seed * 1103515245 + 12345) % (sizeof(salt_chars) - 1)];
    salt[2] = salt_chars[(seed * 2147483647) % (sizeof(salt_chars) - 1)];
    salt[3] = salt_chars[(seed * 1664525 + 1013904223) % (sizeof(salt_chars) - 1)];
    salt[4] = salt_chars[(seed * 48271) % (sizeof(salt_chars) - 1)];
    salt[5] = salt_chars[(seed * 69621 + 1) % (sizeof(salt_chars) - 1)];
    salt[6] = salt_chars[(seed * 16807) % (sizeof(salt_chars) - 1)];
    salt[7] = salt_chars[(seed * 214013 + 2531011) % (sizeof(salt_chars) - 1)];
    salt[8] = '\0';
    
    char salt_buf[32];
    snprintf(salt_buf, sizeof(salt_buf), "$1$%s$", salt);
    
    char *hash = crypt(password, salt_buf);
    if (!hash) {
        fprintf(stderr, "logind: failed to hash password\n");
        return 1;
    }
    
    // Get next UID
    uid_t uid = get_next_uid();
    
    // Create home directory path
    char home_dir[256];
    snprintf(home_dir, sizeof(home_dir), "%s/%s", HOME_DIR_PREFIX, username);
    
    // Create home directory (and parent directories if needed)
    if (create_directory_recursive(home_dir) != 0) {
        fprintf(stderr, "logind: failed to create home directory '%s': %s\n", 
                home_dir, strerror(errno));
        return 1;
    }
    
    // Append to passwd file
    FILE *fp = fopen(PASSWD_FILE, "a");
    if (!fp) {
        fprintf(stderr, "logind: cannot open %s: %s\n", PASSWD_FILE, strerror(errno));
        return 1;
    }
    
    // Format: username:password:uid:gid:gecos:home:shell
    // gid = uid for now (no groups yet)
    // gecos = full name (empty for now)
    // shell = /bin/tsh
    fprintf(fp, "%s:%s:%u:%u::%s:/bin/tsh\n", 
            username, hash, uid, uid, home_dir);
    
    fclose(fp);
    
    printf("logind: user '%s' created successfully (UID: %u)\n", username, uid);
    printf("logind: home directory: %s\n", home_dir);
    
    return 0;
}

static int authenticate_user(const char *username, const char *password) {
    FILE *fp = fopen(PASSWD_FILE, "r");
    if (!fp) {
        fprintf(stderr, "logind: cannot open %s: %s\n", PASSWD_FILE, strerror(errno));
        return 0;
    }
    
    char line[PASSWD_MAX_LINE];
    while (fgets(line, sizeof(line), fp)) {
        // Remove newline
        size_t len = strlen(line);
        if (len > 0 && line[len - 1] == '\n')
            line[len - 1] = '\0';
        
        // Parse passwd entry
        char *fields[7];
        int field_idx = 0;
        char *p = line;
        char *start = p;
        
        while (*p && field_idx < 7) {
            if (*p == ':') {
                *p = '\0';
                fields[field_idx++] = start;
                start = p + 1;
            }
            p++;
        }
        if (field_idx < 7)
            fields[field_idx++] = start;
        
        if (field_idx < 2)
            continue;
        
        // Check username
        if (strcmp(fields[0], username) != 0)
            continue;
        
        // Check password
        char *stored_hash = fields[1];
        char *computed_hash = crypt(password, stored_hash);
        
        if (computed_hash && strcmp(computed_hash, stored_hash) == 0) {
            fclose(fp);
            return 1;
        }
        
        fclose(fp);
        return 0;
    }
    
    fclose(fp);
    return 0;
}

static int do_login(void) {
    char username[USERNAME_MAX + 1];
    
    for (;;) {
        char *password;
        
        printf("Username: ");
        fflush(stdout);
        
        if (!fgets(username, sizeof(username), stdin)) {
            // EOF or error - exit
            if (feof(stdin)) {
                printf("\n");
                return 1;
            }
            fprintf(stderr, "logind: failed to read username\n");
            return 1;
        }
        
        // Remove newline
        size_t len = strlen(username);
        if (len > 0 && username[len - 1] == '\n')
            username[len - 1] = '\0';
        
        if (strlen(username) == 0) {
            fprintf(stderr, "logind: username cannot be empty\n");
            continue; // Retry
        }
        
        password = get_password("Password: ");
        if (!password || strlen(password) == 0) {
            fprintf(stderr, "logind: password cannot be empty\n");
            continue; // Retry
        }
        
        if (!authenticate_user(username, password)) {
            fprintf(stderr, "logind: login failed: invalid username or password\n");
            continue; // Retry login
        }
        
        // Authentication successful - break out of loop
        break;
    }

    // Get user info from passwd file
    FILE *fp = fopen(PASSWD_FILE, "r");
    if (fp) {
        char line[PASSWD_MAX_LINE];
        while (fgets(line, sizeof(line), fp)) {
            // Parse entry
            char *fields[7];
            int field_idx = 0;
            char *p = line;
            char *start = p;
            
            while (*p && field_idx < 7) {
                if (*p == ':') {
                    *p = '\0';
                    fields[field_idx++] = start;
                    start = p + 1;
                }
                p++;
            }
            if (field_idx < 7)
                fields[field_idx++] = start;
            
            if (field_idx >= 3 && strcmp(fields[0], username) == 0) {
                uid_t uid = (uid_t)atoi(fields[2]);
                uid_t gid = (uid_t)atoi(fields[3]);
                char *home = field_idx >= 6 ? fields[5] : HOME_DIR_PREFIX;
                char *shell = field_idx >= 7 ? fields[6] : "/bin/tsh";
                // Set UID/GID (requires root or appropriate privileges)
                if (setgid(gid) != 0) {
                    fprintf(stderr, "logind: warning: failed to setgid: %s\n", strerror(errno));
                }
                if (setuid(uid) != 0) {
                    fprintf(stderr, "logind: warning: failed to setuid: %s\n", strerror(errno));
                }
                
                // Set HOME environment
                char home_env[512];
                snprintf(home_env, sizeof(home_env), "HOME=%s", home);
                putenv(home_env);
                
                // Set USER environment
                char user_env[512];
                snprintf(user_env, sizeof(user_env), "USER=%s", username);
                putenv(user_env);
                
                // Change to home directory
                if (chdir(home) != 0) {
                    // If home doesn't exist, try to create it
                    fprintf(stderr, "logind: warning: home directory '%s' does not exist\n", home);
                }
                
                // Execute shell
                char *argv[] = {shell, NULL};
                char *envp[] = {NULL};
                execve(shell, argv, envp);
                
                fprintf(stderr, "logind: failed to execute shell: %s\n", strerror(errno));
                fclose(fp);
                return 1;
            }
        }
        fclose(fp);
    }
    
    fprintf(stderr, "logind: failed to find user info\n");
    return 1;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        // Default to login
        return do_login();
    }
    
    if (strcmp(argv[1], "-h") == 0 || strcmp(argv[1], "--help") == 0) {
        usage(argv[0]);
        return 0;
    }
    
    if (strcmp(argv[1], "-u") == 0 || strcmp(argv[1], "--user") == 0) {
        if (argc < 3) {
            fprintf(stderr, "logind: --user requires a username\n");
            usage(argv[0]);
            return 1;
        }
        
        const char *username = argv[2];
        char *password = get_password("New password: ");
        
        if (!password || strlen(password) == 0) {
            fprintf(stderr, "logind: password cannot be empty\n");
            return 1;
        }
        
        return create_user(username, password);
    }
    
    if (strcmp(argv[1], "login") == 0) {
        return do_login();
    }
    
    // Default to login if no recognized command
    return do_login();
}

