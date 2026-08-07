----------------------------- MODULE PermitEffect -----------------------------
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS Workers, Directs, EffectfulWorkers, EffectCapacity

ASSUME /\ Workers # {}
       /\ Directs # {}
       /\ Workers \cap Directs = {}
       /\ EffectfulWorkers \subseteq Workers
       /\ EffectCapacity > 0

NoOwner == "none"
Phases == {"idle", "executing", "finished", "settled"}

SeqRange(sequence) == {sequence[index] : index \in 1..Len(sequence)}

VARIABLES permitOwner, waitQueue, phase, effectUsed

vars == <<permitOwner, waitQueue, phase, effectUsed>>

Outstanding ==
    {worker \in Workers : phase[worker] \in {"executing", "finished"}}

Init ==
    /\ permitOwner = NoOwner
    /\ waitQueue = <<>>
    /\ phase = [worker \in Workers |-> "idle"]
    /\ effectUsed = EffectCapacity

AcquireWorker(worker) ==
    /\ worker \in Workers
    /\ phase[worker] = "idle"
    /\ Cardinality(Outstanding) < 1
    /\ permitOwner = NoOwner
    /\ Len(waitQueue) = 0
    /\ permitOwner' = worker
    /\ phase' = [phase EXCEPT ![worker] = "executing"]
    /\ UNCHANGED <<waitQueue, effectUsed>>

QueueDirect(request) ==
    /\ request \in Directs
    /\ request # permitOwner
    /\ request \notin SeqRange(waitQueue)
    /\ waitQueue' = Append(waitQueue, request)
    /\ UNCHANGED <<permitOwner, phase, effectUsed>>

GrantHead ==
    /\ permitOwner = NoOwner
    /\ Len(waitQueue) > 0
    /\ permitOwner' = Head(waitQueue)
    /\ waitQueue' = Tail(waitQueue)
    /\ UNCHANGED <<phase, effectUsed>>

FinishWorker(worker) ==
    /\ worker \in Workers
    /\ permitOwner = worker
    /\ phase[worker] = "executing"
    /\ phase' = [phase EXCEPT ![worker] = "finished"]
    /\ IF Len(waitQueue) = 0
          THEN /\ permitOwner' = NoOwner
               /\ waitQueue' = waitQueue
          ELSE /\ permitOwner' = Head(waitQueue)
               /\ waitQueue' = Tail(waitQueue)
    /\ UNCHANGED effectUsed

ReleaseDirect(request) ==
    /\ request \in Directs
    /\ permitOwner = request
    /\ IF Len(waitQueue) = 0
          THEN /\ permitOwner' = NoOwner
               /\ waitQueue' = waitQueue
          ELSE /\ permitOwner' = Head(waitQueue)
               /\ waitQueue' = Tail(waitQueue)
    /\ UNCHANGED <<phase, effectUsed>>

SettleFinished(worker) ==
    /\ worker \in Workers
    /\ phase[worker] = "finished"
    /\ worker \notin EffectfulWorkers \/ effectUsed < EffectCapacity
    /\ phase' = [phase EXCEPT ![worker] = "settled"]
    /\ effectUsed' = IF worker \in EffectfulWorkers
                         THEN effectUsed + 1
                         ELSE effectUsed
    /\ UNCHANGED <<permitOwner, waitQueue>>

DrainEffect ==
    /\ effectUsed > 0
    /\ effectUsed' = effectUsed - 1
    /\ UNCHANGED <<permitOwner, waitQueue, phase>>

ResetWorker(worker) ==
    /\ worker \in Workers
    /\ phase[worker] = "settled"
    /\ phase' = [phase EXCEPT ![worker] = "idle"]
    /\ UNCHANGED <<permitOwner, waitQueue, effectUsed>>

Next ==
    \/ \E worker \in Workers : AcquireWorker(worker)
    \/ \E request \in Directs : QueueDirect(request)
    \/ GrantHead
    \/ \E worker \in Workers : FinishWorker(worker)
    \/ \E request \in Directs : ReleaseDirect(request)
    \/ \E worker \in Workers : SettleFinished(worker)
    \/ DrainEffect
    \/ \E worker \in Workers : ResetWorker(worker)

TypeOK ==
    /\ permitOwner \in Workers \cup Directs \cup {NoOwner}
    /\ waitQueue \in Seq(Directs)
    /\ Len(waitQueue) = Cardinality(SeqRange(waitQueue))
    /\ permitOwner \notin SeqRange(waitQueue)
    /\ phase \in [Workers -> Phases]
    /\ effectUsed \in 0..EffectCapacity
    /\ \A worker \in Workers :
           (phase[worker] = "executing") <=> (permitOwner = worker)
    /\ \A worker \in Workers :
           phase[worker] = "finished" => permitOwner # worker

WorkerSlotBound == Cardinality(Outstanding) <= 1

FinishedDoesNotRetainPermit ==
    \A worker \in Workers : phase[worker] = "finished" => permitOwner # worker

FinishedEventuallySettles ==
    \A worker \in Workers :
        (phase[worker] = "finished") ~> (phase[worker] = "settled")

QueuedDirectEventuallyOwnsPermit ==
    \A request \in Directs :
        (request \in SeqRange(waitQueue)) ~> (permitOwner = request)

\* This deliberately false invariant is used by the reachability configuration
\* to prove the exact effect-blocked/direct-handoff state is not vacuous.
NoEffectBlockedDirectHandoff ==
    ~\E worker \in Workers :
        /\ phase[worker] = "finished"
        /\ effectUsed = EffectCapacity
        /\ permitOwner \in Directs

Spec ==
    /\ Init
    /\ [][Next]_vars
    /\ WF_vars(GrantHead)
    /\ WF_vars(DrainEffect)
    /\ \A worker \in Workers : WF_vars(FinishWorker(worker))
    /\ \A worker \in Workers : WF_vars(SettleFinished(worker))
    /\ \A request \in Directs : WF_vars(ReleaseDirect(request))

=============================================================================
