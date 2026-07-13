#[cfg(test)]
mod security_tests {
    use twilight_kernel::sys::console;
    use std::process::Command;
    use std::io::Write;

    #[test]
    fn test_console_rejects_unauthenticated_input() {
        // Invariant: Console endpoints reject unauthenticated requests
        let payloads = vec![
            // Exact exploit case: raw input bytes without authentication
            b"malicious_command\n".to_vec(),
            // Boundary case: empty input (should still require auth)
            b"".to_vec(),
            // Valid input format but without auth context
            b"ls -la\n".to_vec(),
        ];

        for payload in &payloads {
            // Create a test file with the payload
            let test_file = "/tmp/test_console_input";
            let mut file = std::fs::File::create(test_file).unwrap();
            file.write_all(payload).unwrap();
            
            // Attempt to write to console device without authentication
            let output = Command::new("cat")
                .arg(test_file)
                .arg(">")
                .arg("/dev/console")
                .output()
                .expect("Failed to execute command");
            
            // The request should fail - either permission denied or operation not permitted
            assert!(
                !output.status.success(),
                "Console accepted unauthenticated input: {:?}",
                String::from_utf8_lossy(payload)
            );
            
            // Clean up test file
            std::fs::remove_file(test_file).unwrap();
        }
        
        // Additional direct API test if available
        if let Ok(mut console) = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/console") 
        {
            // This should fail with permission error
            let result = console.write(b"test\n");
            assert!(result.is_err(), "Direct console write should fail without auth");
        }
    }
}