//! 统一启动的原子性、初始队列快照与首轮注入边界。

#[path = "support/mock.rs"]
mod mock;

use futures::StreamExt;
use mock::{text, Script, ScriptedModel};
use std::sync::{Arc, Barrier};
use zach_agent::{Agent, AgentError, AgentRun, QueueMode, RetryPolicy};
use zach_ai_core::Message;

fn agent(model: Arc<ScriptedModel>, mode: QueueMode) -> Agent {
    let agent = Agent::builder(model).retry(RetryPolicy::none()).build();
    agent.set_steering_mode(mode);
    agent.set_follow_up_mode(mode);
    agent
}

/// 覆盖新提示、助手尾继续和用户尾继续三个启动入口。
fn start(agent: &Agent, kind: usize) -> Result<AgentRun, AgentError> {
    if kind == 0 {
        agent.prompt_text("新问题")
    } else {
        agent.continue_run()
    }
}

fn history(kind: usize) -> Vec<Message> {
    match kind {
        0 => vec![],
        1 => vec![Message::user("旧问题"), Message::assistant("旧回答")],
        _ => vec![Message::user("待回答")],
    }
}

#[tokio::test]
async fn initial_steering_is_drained_synchronously_once_per_turn() {
    for mode in [QueueMode::OneAtATime, QueueMode::All] {
        for kind in 0..3 {
            let model = ScriptedModel::new(vec![
                Script::Parts(text("第一答")),
                Script::Parts(text("第二答")),
            ]);
            let agent = agent(model.clone(), mode);
            let mut first = history(kind);
            agent.set_messages(first.clone());
            if kind == 0 {
                first.push(Message::user("新问题"));
            }
            let queued = vec![Message::user("插队甲"), Message::user("插队乙")];
            for message in &queued {
                agent.steer(message.clone());
            }

            let run = start(&agent, kind).unwrap();
            // 当前线程尚未让出执行权：消费必须发生在 start 内，而非后台首轮。
            let remaining = if mode == QueueMode::All {
                vec![]
            } else {
                queued[1..].to_vec()
            };
            assert_eq!(agent.peek_queued_messages(), remaining);
            let take = if mode == QueueMode::All { 2 } else { 1 };
            first.extend_from_slice(&queued[..take]);

            let output = run.outcome().await.unwrap();
            assert_eq!(model.call(0).prompt.messages, first);
            assert_eq!(
                model.call_count(),
                if mode == QueueMode::All { 1 } else { 2 }
            );
            if mode == QueueMode::OneAtATime {
                first.push(Message::assistant("第一答"));
                first.push(queued[1].clone());
                assert_eq!(model.call(1).prompt.messages, first);
            }
            for message in queued {
                assert_eq!(output.messages.iter().filter(|m| **m == message).count(), 1);
            }
            assert!(!agent.has_queued_messages());
        }
    }
}

#[tokio::test]
async fn normal_prompt_keeps_follow_up_until_after_the_first_response() {
    for mode in [QueueMode::OneAtATime, QueueMode::All] {
        let model = ScriptedModel::new(vec![Script::Parts(text("首答"))]);
        let agent = agent(model.clone(), mode);
        let queued = vec![Message::user("追加甲"), Message::user("追加乙")];
        for message in &queued {
            agent.follow_up(message.clone());
        }
        let run = agent.prompt_text("新问题").unwrap();
        assert_eq!(
            agent.peek_queued_messages(),
            queued[..if mode == QueueMode::All { 2 } else { 1 }]
        );
        run.outcome().await.unwrap();
        assert_eq!(model.call(0).prompt.messages, vec![Message::user("新问题")]);
        assert_eq!(
            model.call_count(),
            if mode == QueueMode::All { 2 } else { 3 }
        );
        assert_eq!(
            model.call(1).prompt.messages[2..],
            queued[..if mode == QueueMode::All { 2 } else { 1 }]
        );
        assert!(!agent.has_queued_messages());
    }
}

#[tokio::test]
async fn steering_added_after_start_is_reserved_for_the_next_turn() {
    for mode in [QueueMode::OneAtATime, QueueMode::All] {
        for kind in 0..3 {
            let model = ScriptedModel::new(vec![
                Script::Parts(text("首答")),
                Script::Parts(text("次答")),
            ]);
            let agent = agent(model.clone(), mode);
            agent.set_messages(history(kind));
            let initial = Message::user("初始插队");
            let late = Message::user("启动之后插队");
            agent.steer(initial.clone());
            let run = start(&agent, kind).unwrap();
            assert!(!agent.has_queued_messages());
            // 无 await/yield：后台尚未运行，但首轮输入的快照已经封闭。
            agent.steer(late.clone());
            run.outcome().await.unwrap();
            assert_eq!(model.call_count(), 2);
            let first = model.call(0).prompt.messages;
            assert_eq!(first.last(), Some(&initial));
            assert!(!first.contains(&late));
            assert_eq!(model.call(1).prompt.messages.last(), Some(&late));
            assert_eq!(agent.messages().iter().filter(|m| **m == late).count(), 1);
        }
    }
}

#[tokio::test]
async fn assistant_tail_falls_back_to_follow_up_without_repolling_initial_steering() {
    for mode in [QueueMode::OneAtATime, QueueMode::All] {
        let model = ScriptedModel::new(vec![Script::Parts(text("首答"))]);
        let agent = agent(model.clone(), mode);
        let mut first = history(1);
        agent.set_messages(first.clone());
        let queued = vec![Message::user("追加甲"), Message::user("追加乙")];
        for message in &queued {
            agent.follow_up(message.clone());
        }
        let run = agent.continue_run().unwrap();
        let take = if mode == QueueMode::All { 2 } else { 1 };
        assert_eq!(agent.peek_queued_messages(), queued[take..]);
        first.extend_from_slice(&queued[..take]);
        let late = Message::user("启动后插队");
        agent.steer(late.clone());
        run.outcome().await.unwrap();
        assert_eq!(model.call(0).prompt.messages, first);
        assert_eq!(model.call(1).prompt.messages.last(), Some(&late));
        assert_eq!(
            model.call_count(),
            if mode == QueueMode::All { 2 } else { 3 }
        );
        assert!(!agent.has_queued_messages());
    }
}

#[tokio::test]
async fn rejected_starts_leave_queues_untouched() {
    for mode in [QueueMode::OneAtATime, QueueMode::All] {
        let model = ScriptedModel::new(vec![Script::Hang(vec![])]);
        let agent = agent(model.clone(), mode);
        agent.steer(Message::user("插队"));
        agent.follow_up(Message::user("追加"));
        assert!(matches!(agent.continue_run(), Err(AgentError::NoMessages)));
        assert_eq!(agent.peek_queued_messages(), vec![Message::user("插队")]);
        agent.clear_steering_queue();
        assert_eq!(agent.peek_queued_messages(), vec![Message::user("追加")]);
        agent.clear_queues();
        agent.set_messages(history(1));
        assert!(matches!(
            agent.continue_run(),
            Err(AgentError::CannotContinueFromAssistant)
        ));
        assert!(!agent.has_queued_messages());
        assert!(!agent.is_running());
        assert_eq!(model.call_count(), 0);

        let run = agent.prompt_text("挂起").unwrap();
        agent.steer(Message::user("忙时插队"));
        agent.follow_up(Message::user("忙时追加"));
        assert!(matches!(agent.prompt_text("拒绝"), Err(AgentError::Busy)));
        assert!(matches!(agent.continue_run(), Err(AgentError::Busy)));
        assert_eq!(
            agent.peek_queued_messages(),
            vec![Message::user("忙时插队")]
        );
        agent.clear_steering_queue();
        assert_eq!(
            agent.peek_queued_messages(),
            vec![Message::user("忙时追加")]
        );
        run.abort();
        assert!(run.outcome().await.unwrap().aborted);
    }
}

/// 两个原生线程同时进入启动；挂起模型保证赢家在两次调用返回前不会结束。
#[tokio::test]
async fn competing_starts_have_one_winner_and_never_lose_queued_messages() {
    for both_continue in [false, true] {
        for follow_up in [false, true] {
            for mode in [QueueMode::OneAtATime, QueueMode::All] {
                for _ in 0..8 {
                    let model = ScriptedModel::new(vec![Script::Hang(vec![])]);
                    let agent = agent(model.clone(), mode);
                    agent.set_messages(history(1));
                    let queued = vec![Message::user("竞争甲"), Message::user("竞争乙")];
                    for message in &queued {
                        if follow_up {
                            agent.follow_up(message.clone());
                        } else {
                            agent.steer(message.clone());
                        }
                    }
                    let barrier = Arc::new(Barrier::new(3));
                    let runtime = tokio::runtime::Handle::current();
                    let results = std::thread::scope(|scope| {
                        let mut threads = Vec::new();
                        for index in 0..2 {
                            let (agent, barrier, runtime) =
                                (agent.clone(), barrier.clone(), runtime.clone());
                            threads.push(scope.spawn(move || {
                                let _entered = runtime.enter();
                                barrier.wait();
                                if both_continue || index == 1 {
                                    agent.continue_run()
                                } else {
                                    agent.prompt_text("并发新问题")
                                }
                            }));
                        }
                        barrier.wait();
                        threads
                            .into_iter()
                            .map(|thread| thread.join().unwrap())
                            .collect::<Vec<_>>()
                    });
                    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
                    assert_eq!(
                        results
                            .iter()
                            .filter(|result| matches!(result, Err(AgentError::Busy)))
                            .count(),
                        1
                    );
                    assert!(agent.is_running());
                    let mut run = results.into_iter().find_map(Result::ok).unwrap();
                    // 明确等待模型接管快照，不使用 sleep 猜测任务调度。
                    tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        while model.call_count() == 0 {
                            assert!(run.next().await.is_some());
                        }
                    })
                    .await
                    .expect("赢家应开始首次模型请求");
                    assert!(agent.is_running());
                    assert_eq!(model.call_count(), 1);
                    agent.set_steering_mode(QueueMode::All);
                    agent.set_follow_up_mode(QueueMode::All);
                    let remaining = agent.peek_queued_messages();
                    let request = model.call(0).prompt.messages;
                    for message in &queued {
                        let taken = request.iter().filter(|m| *m == message).count();
                        let waiting = remaining.iter().filter(|m| *m == message).count();
                        assert_eq!(taken + waiting, 1, "排队消息必须由赢家接管或保留");
                    }
                    if !follow_up {
                        assert!(
                            request.contains(&queued[0]),
                            "prompt 与 continue 都应同步接管初始插队"
                        );
                    }
                    run.abort();
                    assert!(run.outcome().await.unwrap().aborted);
                }
            }
        }
    }
}
