Block Checking Flow: Queue Fairness First, Ledger Priority Later

  ╭──────────────────────────╮
  │ Peer sends Publish       │
  ╰────────────┬─────────────╯
               │ decoded from TCP session
               ▼
  ╔══════════════════════════╗
  ║ InboundMessageQueue      ║
  ╚═══════════╤══════════════╝
              │ put(message, channel)
              ▼
  ┌──────────────────────────┐
  │ FairQueue<ChannelId,msg> │
  └───────────┬──────────────┘
              │ key = channel_id
              │ max = message_processor.max_queue
              │ priority = 1 for every channel
              ▼
  ┌──────────────────────────┐
  │ per-channel subqueues    │
  └──────┬────────────┬──────┘
         │            │
         │            └──────────────────────► one peer cannot monopolize inbound message work
         │
         │ next_batch(4096)
         ▼
  ╔══════════════════════════╗
  ║ MessageProcessor         ║
  ╚═══════════╤══════════════╝
              │ worker threads call processor.process(message, channel)
              ▼
  ╔══════════════════════════╗
  ║ NetworkMessageProcessor  ║
  ╚═══════════╤══════════════╝
              │ Message::Publish
              ▼
  ┌──────────────────────────┐
  │ work precheck            │
  └──────┬────────────┬──────┘
         │ ok         │ invalid work
         │            ▼
         │     ┌──────────────────────────┐
         │     │ Drop::Publish            │
         │     └──────────────────────────┘
         ▼
  ┌──────────────────────────┐
  │ bootstrap live gate      │
  └──────┬────────────┬──────┘
         │ not bootstrapping │ bootstrapping
         │                   ▼
         │            ┌──────────────────────────┐
         │            │ drop live block          │
         │            └──────────────────────────┘
         ▼
  ┌──────────────────────────┐
  │ choose BlockSource       │
  └──────┬────────────┬──────┐
         │            │      │
         │            │      └──────────── publish.is_originator=false
         │            │                     source = Live
         │            └─────────────────── publish.is_originator=true
         │                                  source = LiveOriginator
         ▼
  ╔══════════════════════════╗
  ║ BlockContext             ║
  ║ block + source + channel ║
  ╚═══════════╤══════════════╝
              │ push(context)
              ▼
  ╔══════════════════════════╗
  ║ BlockProcessorQueue      ║
  ╚═══════════╤══════════════╝
              │ wraps ProcessQueue
              ▼
  ┌───────────────────────────────────────────────────────────────────────────────┐
  │ FairQueue<(BlockSource,ChannelId), BlockContext>                              │
  └───────────┬───────────────────────────────────────────────────────────────────┘
              │ subqueue is created when first item for (source,channel) arrives
              │ max_size_query(source):
              │   Live / LiveOriginator  -> max_peer_queue      default 1024
              │   Bootstrap/Unchecked/Local/Forced -> max_system_queue default 16384
              │ priority_query(source):
              │   Live / LiveOriginator  -> 1
              │   Bootstrap / Unchecked  -> 8
              │   Local                  -> 16
              │   Forced                 -> 32
              ▼
  ┌──────────────────────────────────────────────────────────────────────────────┐
  │ Block processor fairness decision happens here, before ledger validation     │
  └──────┬────────────────────────────┬───────────────────────────────┬──────────┘
         │                            │                               │
         ▼                            ▼                               ▼
  ┌──────────────────────┐    ┌──────────────────────┐       ┌──────────────────────┐
  │ Live peer queues     │    │ Bootstrap queues     │       │ Forced queue         │
  │ share = 1 each       │    │ share = 8 each       │       │ share = 32           │
  └──────────┬───────────┘    └──────────┬───────────┘       └──────────┬───────────┘
             │                           │                              │
             │ same source fairness      │ bootstrap gets more turns     │ fork winner change path
             │ still split by channel    │ than live peers               │ ChannelId::LOOPBACK
             ▼                           ▼                              ▼
  ┌───────────────────────────────────────────────────────────────────────────────┐
  │ FairQueue pop(): stay on current subqueue until counter >= queue.priority     │
  └───────────────────────────────────────────────────────────────────────────────┘
              │
              │ example with queued work:
              │   Forced can pop up to 32 before seek_next()
              │   Local can pop up to 16 before seek_next()
              │   Bootstrap can pop up to 8 before seek_next()
              │   Live can pop 1 before seek_next()
              │
              │ this is source/channel service share, not consensus priority
              ▼
  ╔══════════════════════════╗
  ║ BlockProcessorLoop       ║
  ╚═══════════╤══════════════╝
              │ pop_blocking()
              ▼
  ┌──────────────────────────┐
  │ ProcessQueue.next_batch  │
  │ default batch_size 256   │
  └───────────┬──────────────┘
              │ maybe throttle if bounded backlog says slow down
              ▼
  ╔══════════════════════════╗
  ║ BlockBatchProcessor      ║
  ╚═══════════╤══════════════╝
              │ roll_back_competitor_blocks(for Forced only)
              ▼
  ╔══════════════════════════╗
  ║ Ledger.process_batch     ║
  ╚═══════════╤══════════════╝
              │ read transaction: validate each block
              ▼
  ┌──────────────────────────┐
  │ BlockValidatorFactory    │
  └───────────┬──────────────┘
              │ builds BlockValidator from ledger state
              ▼
  ┌──────────────────────────┐
  │ BlockValidator.validate  │
  └───────────┬────────────────────────────────────────────────────────────────────┐
              │ checks: exists, predecessor, signature, burn account, account open │
              │ checks: pending receive, work, negative spend, epoch rules         │
              ▼                                                                    │
  ┌──────────────────────────┐                                                     │
  │ create instructions      │                                                     │
  └───────────┬──────────────┘                                                     │
              │ FIRST consensus BlockPriority decision                             │
              │ block_priority_sideband(sideband, previous_block)                  │
              │   priority.balance = max(new balance, previous balance if send)    │
              │   priority.time = previous timestamp, else current sideband time   │
              ▼                                                                    │
  ┌──────────────────────────┐                                                     │
  │ BlockInsertInstructions  │                                                     │
  │ includes priority        │                                                     │
  └───────────┬────────────────────────────────────────────────────────────────────┘
              │ write transaction: insert valid blocks
              ▼
  ┌──────────────────────────┐
  │ BlockInserter.insert     │
  └──────┬────────────┬──────┘
         │ ok         │ conflict / validation error
         │            ▼
         │     ┌──────────────────────────┐
         │     │ ProcessResult error      │
         │     │ priority = default       │
         │     └──────────┬───────────────┘
         │                │ GapPrevious / GapSource
         │                ▼
         │     ┌──────────────────────────┐
         │     │ UncheckedMap             │
         │     │ wait for dependency      │
         │     └──────────┬───────────────┘
         │                │ dependency later inserted
         │                ▼
         │     ┌──────────────────────────┐
         │     │ UncheckedReenqueuer      │
         │     │ source = Unchecked       │
         │     └──────────┬───────────────┘
         │                │ back to BlockProcessorQueue
         │                └───────────────────────────────────────────────┐
         ▼                                                                │
  ┌──────────────────────────┐                                            │
  │ stores SavedBlock        │                                            │
  └───────────┬──────────────┘                                            │
              │ writes block/account/pending/successor/rep weights/cache  │
              ▼                                                           │
  ┌──────────────────────────┐                                            │
  │ ProcessResult ok         │                                            │
  │ saved_block + priority   │                                            │
  └───────────┬───────────────────────────────────────────────────────────┘
              │ Ledger.notify(BlocksProcessed(results))
              ▼
  ╔══════════════════════════╗
  ║ LedgerPipelineEvent      ║
  ║ BlocksProcessed          ║
  ╚════╤══════════════╤══════════════╤══════════════╤══════════════════════════════╗
       │              │              │              │                              ║
       ▼              ▼              ▼              ▼                              ║
  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────────────────────┐  ║
  │ ConfirmingSet│ │ ForkCache    │ │ Bootstrapper │ │ LocalBlockBroadcaster    │  ║
  └──────────────┘ └──────────────┘ └──────┬───────┘ └──────────────────────────┘  ║
                                           │                                       ║
                                           │ may enqueue next bootstrap block      ║
                                           ▼                                       ║
                                  ┌──────────────────────────┐                     ║
                                  │ BlockSource::Bootstrap   │                     ║
                                  └──────────┬───────────────┘                     ║
                                             └──── back to BlockProcessorQueue ◄───╝
       │
       │ EventHandler<LedgerPipelineEvent>
       ▼
  ╔══════════════════════════╗
  ║ ElectionSchedulers       ║
  ╚═══════════╤══════════════╝
              │ for each ok ProcessResult
              │ account = saved_block.account()
              ▼
  ┌──────────────────────────┐
  │ prio_sched_queue         │
  │ EventProcessor<Account>  │
  └───────────┬──────────────┘
              │ async activation worker
              ▼
  ╔══════════════════════════╗
  ║ PriorityScheduler        ║
  ╚═══════════╤══════════════╝
              │ activate(any, account)
              ▼
  ┌──────────────────────────┐
  │ account has backlog?     │
  └──────┬────────────┬──────┘
         │ yes        │ no
         │            ▼
         │     ┌──────────────────────────┐
         │     │ ActivateSkip stat        │
         │     └──────────────────────────┘
         ▼
  ┌──────────────────────────┐
  │ next unconfirmed hash    │
  └───────────┬──────────────┘
              │ if conf height 0 -> open block
              │ else successor of confirmed frontier
              ▼
  ┌──────────────────────────┐
  │ dependency gate          │
  └──────┬────────────┬──────┐
         │ confirmed  │ not confirmed / forked / missing
         │            ▼
         │     ┌──────────────────────────┐
         │     │ ActivateFailed / return  │
         │     └──────────────────────────┘
         ▼
  ┌──────────────────────────┐
  │ any.block_priority       │
  └───────────┬──────────────┘
              │ SECOND BlockPriority calculation from stored SavedBlock
              │ same balance/time rules as validation
              ▼
  ╔══════════════════════════╗
  ║ PriorityBuckets          ║
  ╚═══════════╤══════════════╝
              │ bucket = prio_bucket_index(priority.balance)
              │ 63 priority buckets by balance minimums
              ▼
  ┌──────────────────────────┐
  │ Bucket.insert            │
  └──────┬────────────┬──────────────────────────────┬────────────────────────────┐
         │ room       │ duplicate                    │ full and too low           │ full and higher
         ▼            ▼                              ▼                            ▼
  ┌──────────────┐ ┌──────────────┐          ┌───────────────┐            ┌──────────────┐
  │ queued block │ │ Duplicate    │          │ PriorityTooLow│            │ evict lowest │
  └──────┬───────┘ └──────────────┘          └───────────────┘            └──────┬───────┘
         │                                                                    queued
         └───────────────────────────────┬───────────────────────────────────────┘
                                         │ notify scheduler condition
                                         ▼
  ╔══════════════════════════╗
  ║ PriorityScheduler thread ║
  ╚═══════════╤══════════════╝
              │ predicate: AEC vacancy check against PriorityBuckets
              ▼
  ╔══════════════════════════╗
  ║ AecService.refill        ║
  ╚═══════════╤══════════════╝
              │ iterates bucket ids from high to low
              │ asks source.next_candidate(bucket_id, vacancy, lowest_priority)
              ▼
  ┌──────────────────────────┐
  │ Bucket.available         │
  └──────┬─────────────────────────────────────────────┬──────────────────────────┐
         │ vacancy > 0                                 │ no vacancy               │
         │                                             │ candidate.time > lowest active time
         ▼                                             ▼
  ┌──────────────────────────┐                ┌───────────────────────────┐
  │ pop highest queued block │                │ replace low-prio election │
  └───────────┬──────────────┘                └───────────┬───────────────┘
              │                                           │
              └──────────────────────┬────────────────────┘
                                     ▼
  ┌──────────────────────────┐
  │ AecInsertRequest priority│
  └───────────┬──────────────┘
              │ behavior = Priority
              ▼
  ╔══════════════════════════╗
  ║ ActiveElectionsContainer ║
  ╚═══════════╤══════════════╝
              │ insert election by qualified root
              │ bucket_index(ElectionBehavior::Priority, priority.balance)
              ▼
  ┌──────────────────────────┐
  │ RootContainer buckets    │
  └───────────┬──────────────┘
              │ active elections ordered by priority.time, then priority.balance
              ▼
  ╔══════════════════════════╗
  ║ Active Election          ║
  ╚═══════════╤══════════════╝
              │ vote processing, confirmation, cleanup
              ▼
  ╔══════════════════════════╗
  ║ AecFactProcessor         ║
  ╚════╤═════════════════════╝
       │ ElectionEnded / Recovered
       ▼
  ┌──────────────────────────┐
  │ schedulers.notify        │
  └──────────────────────────┘
       │
       └──── gives PriorityScheduler another chance to refill vacancies

  HIGH-PRIORITY COMPARED WITH LOW-PRIORITY
  ────────────────────────────────────────────────────────────────────────────────────────────
  Before ledger:
    high balance does not help in BlockProcessorQueue.
    only BlockSource share matters:
      Forced(32) > Local(16) > Bootstrap/Unchecked(8) > Live/LiveOriginator(1).

  After ledger:
    balance/time priority starts to matter.
    high balance maps to a higher priority bucket.
    within buckets, older time priority wins.
    if buckets/elections are full, higher priority can evict or replace lower priority.

  Key point: the block processor does not know “high balance priority” when it accepts a publish. It first schedules by BlockSource fairness; the consensus/election priority is computed only after
  validation/insert produces a SavedBlock and BlockPriority.