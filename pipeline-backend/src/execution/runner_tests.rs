// Tests for `ExecutionRunner`.
//
// This module is included by `execution/mod.rs` as `#[cfg(test)] mod runner_tests;`,
// so the tests live at this module's top level. Referencing `crate::execution::ExecutionRunner`
// explicitly avoids the previous `use super::*` bug where `super` resolved to this
// test module (which does not re-export `ExecutionRunner`).

use crate::execution::ExecutionRunner;

#[tokio::test]
async fn test_memory_events_push() {
    let runner = ExecutionRunner::new();

    // Create simple PUSH binary
    // mov rax, 0x1234567890ABCDEF
    // push rax
    // ret
    let binary = vec![
        0x48, 0xB8, 0xEF, 0xCD, 0xAB, 0x90, 0x78, 0x56, 0x34, 0x12, // mov rax, immediate
        0x50, // push rax
        0xC3, // ret
    ];

    let frames = runner.run(&binary).await.unwrap();

    // Should have at least 3 frames: mov, push, ret
    assert!(frames.len() >= 3);

    // Find PUSH frame (should be second instruction)
    let push_frame = &frames[1];

    // Verify memory events were created
    assert!(!push_frame.memory_events.is_empty(), "PUSH should create memory event");

    // Verify PUSH memory event is a write
    let push_event = &push_frame.memory_events[0];
    assert!(push_event.is_write, "PUSH should be a write operation");
    assert_eq!(push_event.size, 8, "PUSH should write 8 bytes");
    assert_eq!(push_event.value, 0x1234567890ABCDEF, "PUSH value should match");
}

#[tokio::test]
async fn test_mov_instruction() {
    let runner = ExecutionRunner::new();

    // mov rax, 10
    // mov rbx, rax
    // ret
    let binary = vec![
        0x48, 0xC7, 0xC0, 0x0A, 0x00, 0x00, 0x00, // mov rax, 10
        0x48, 0x89, 0xC3,                         // mov rbx, rax
        0xC3,                                     // ret
    ];

    let frames = runner.run(&binary).await.unwrap();

    // After second MOV, RBX should equal RAX
    let final_frame = &frames[frames.len() - 1];
    let rax_value = final_frame.registers.iter()
        .find(|(name, _)| name == "rax")
        .map(|(_, value)| *value)
        .unwrap();
    let rbx_value = final_frame.registers.iter()
        .find(|(name, _)| name == "rbx")
        .map(|(_, value)| *value)
        .unwrap();

    assert_eq!(rax_value, 10, "RAX should be 10");
    assert_eq!(rbx_value, 10, "RBX should equal RAX");
    assert_eq!(rax_value, rbx_value, "RAX and RBX should be equal");
}

#[tokio::test]
async fn test_add_instruction() {
    let runner = ExecutionRunner::new();

    // mov rax, 5
    // mov rbx, 10
    // add rax, rbx
    // ret
    let binary = vec![
        0x48, 0xC7, 0xC0, 0x05, 0x00, 0x00, 0x00, // mov rax, 5
        0x48, 0xC7, 0xC3, 0x0A, 0x00, 0x00, 0x00, // mov rbx, 10
        0x48, 0x01, 0xD8,                         // add rax, rbx
        0xC3,                                     // ret
    ];

    let frames = runner.run(&binary).await.unwrap();

    let final_frame = &frames[frames.len() - 1];
    let rax_value = final_frame.registers.iter()
        .find(|(name, _)| name == "rax")
        .map(|(_, value)| *value)
        .unwrap();

    assert_eq!(rax_value, 15, "RAX should be 5 + 10 = 15");
}

#[tokio::test]
async fn test_sub_instruction() {
    let runner = ExecutionRunner::new();

    // mov rax, 20
    // mov rbx, 7
    // sub rax, rbx
    // ret
    let binary = vec![
        0x48, 0xC7, 0xC0, 0x14, 0x00, 0x00, 0x00, // mov rax, 20
        0x48, 0xC7, 0xC3, 0x07, 0x00, 0x00, 0x00, // mov rbx, 7
        0x48, 0x29, 0xD8,                         // sub rax, rbx
        0xC3,                                     // ret
    ];

    let frames = runner.run(&binary).await.unwrap();

    let final_frame = &frames[frames.len() - 1];
    let rax_value = final_frame.registers.iter()
        .find(|(name, _)| name == "rax")
        .map(|(_, value)| *value)
        .unwrap();

    assert_eq!(rax_value, 13, "RAX should be 20 - 7 = 13");
}

#[tokio::test]
async fn test_inc_dec_instructions() {
    let runner = ExecutionRunner::new();

    // mov rax, 10
    // inc rax
    // dec rax
    // ret
    let binary = vec![
        0x48, 0xC7, 0xC0, 0x0A, 0x00, 0x00, 0x00, // mov rax, 10
        0x48, 0xFF, 0xC0,                         // inc rax
        0x48, 0xFF, 0xC8,                         // dec rax
        0xC3,                                     // ret
    ];

    let frames = runner.run(&binary).await.unwrap();

    let final_frame = &frames[frames.len() - 1];
    let rax_value = final_frame.registers.iter()
        .find(|(name, _)| name == "rax")
        .map(|(_, value)| *value)
        .unwrap();

    // 10 + 1 - 1 = 10
    assert_eq!(rax_value, 10, "RAX should be back to 10 after INC and DEC");
}

#[tokio::test]
async fn test_call_ret_instructions() {
    let runner = ExecutionRunner::new();

    // Simple call/ret test
    // mov rax, 5
    // call label
    // ret
    // label: inc rax
    // ret
    let binary = vec![
        0x48, 0xC7, 0xC0, 0x05, 0x00, 0x00, 0x00, // mov rax, 5
        0xE8, 0x02, 0x00, 0x00, 0x00,             // call +2
        0xC3,                                     // ret
        0x48, 0xFF, 0xC0,                         // inc rax (label)
        0xC3,                                     // ret
    ];

    let frames = runner.run(&binary).await.unwrap();

    // Should have stack frames after CALL
    let frame_with_stack = frames.iter()
        .find(|f| !f.stack.is_empty());

    assert!(frame_with_stack.is_some(), "CALL should create stack frame");

    // Should have memory events for CALL (return address write)
    let frame_with_call_event = frames.iter()
        .find(|f| f.memory_events.iter().any(|e| e.is_write));

    assert!(frame_with_call_event.is_some(), "CALL should create memory write event");
}
