----------------------------- MODULE PermitEffect -----------------------------
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS Workers, Directs, EffectfulWorkers,
          PermitCapacity, WorkerCapacity, EffectCapacity

ASSUME /\ Workers # {}
       /\ Directs # {}
       /\ Workers \cap Directs = {}
       /\ EffectfulWorkers \subseteq Workers
       /\ PermitCapacity > 0
       /\ WorkerCapacity > 0
       /\ WorkerCapacity <= Cardinality(Workers)
       /\ EffectCapacity > 0

Phases == {"idle", "executing", "finished", "settled"}

SeqRange(sequence) == {sequence[index] : index \in 1..Len(sequence)}

SeqPosition(sequence, member) ==
    CHOOSE index \in 1..Len(sequence) : sequence[index] = member

RemoveSeqMember(sequence, member) ==
    [index \in 1..(Len(sequence) - 1) |->
        IF index < SeqPosition(sequence, member)
           THEN sequence[index]
           ELSE sequence[index + 1]]

VARIABLES permitOwners, waitQueue, finishedQueue, phase, effectUsed, ingressOpen

vars == <<permitOwners, waitQueue, finishedQueue, phase, effectUsed, ingressOpen>>

Outstanding ==
    {worker \in Workers : phase[worker] \in {"executing", "finished"}}

Init ==
    /\ permitOwners = {}
    /\ waitQueue = <<>>
    /\ finishedQueue = <<>>
    /\ phase = [worker \in Workers |-> "idle"]
    /\ effectUsed = EffectCapacity
    /\ ingressOpen = TRUE

AcquireWorker(worker) ==
    /\ worker \in Workers
    /\ phase[worker] = "idle"
    /\ Cardinality(Outstanding) < WorkerCapacity
    /\ Cardinality(permitOwners) < PermitCapacity
    /\ Len(waitQueue) = 0
    /\ permitOwners' = permitOwners \cup {worker}
    /\ phase' = [phase EXCEPT ![worker] = "executing"]
    /\ UNCHANGED <<waitQueue, finishedQueue, effectUsed, ingressOpen>>

QueueDirect(request) ==
    /\ request \in Directs
    /\ request \notin permitOwners
    /\ request \notin SeqRange(waitQueue)
    /\ waitQueue' = Append(waitQueue, request)
    /\ UNCHANGED <<permitOwners, finishedQueue, phase, effectUsed, ingressOpen>>

GrantHead ==
    /\ Len(waitQueue) > 0
    /\ Cardinality(permitOwners) < PermitCapacity
    /\ permitOwners' = permitOwners \cup {Head(waitQueue)}
    /\ waitQueue' = Tail(waitQueue)
    /\ UNCHANGED <<finishedQueue, phase, effectUsed, ingressOpen>>

FinishWorker(worker) ==
    /\ worker \in Workers
    /\ worker \in permitOwners
    /\ phase[worker] = "executing"
    /\ phase' = [phase EXCEPT ![worker] = "finished"]
    /\ finishedQueue' = Append(finishedQueue, worker)
    /\ IF Len(waitQueue) = 0
          THEN /\ permitOwners' = permitOwners \ {worker}
               /\ waitQueue' = waitQueue
          ELSE /\ permitOwners' = (permitOwners \ {worker}) \cup {Head(waitQueue)}
               /\ waitQueue' = Tail(waitQueue)
    /\ UNCHANGED <<effectUsed, ingressOpen>>

ReleaseDirect(request) ==
    /\ request \in Directs
    /\ request \in permitOwners
    /\ IF Len(waitQueue) = 0
          THEN /\ permitOwners' = permitOwners \ {request}
               /\ waitQueue' = waitQueue
          ELSE /\ permitOwners' = (permitOwners \ {request}) \cup {Head(waitQueue)}
               /\ waitQueue' = Tail(waitQueue)
    /\ UNCHANGED <<finishedQueue, phase, effectUsed, ingressOpen>>

\* `finishedQueue` is the finite abstraction of the selected kernel's existing
\* monotonic capability-id rank. It is not a proposed production queue. An
\* effectful completion cannot overtake an older effectful completion at the
\* next freed capacity cut. A no-effect completion may bypass blocked effectful
\* work because it consumes no contested journal resource.
SettleFinished(worker) ==
    /\ worker \in SeqRange(finishedQueue)
    /\ phase[worker] = "finished"
    /\ IF worker \in EffectfulWorkers
          THEN /\ effectUsed < EffectCapacity
               /\ \A index \in 1..(SeqPosition(finishedQueue, worker) - 1) :
                      finishedQueue[index] \notin EffectfulWorkers
          ELSE TRUE
    /\ phase' = [phase EXCEPT ![worker] = "settled"]
    /\ finishedQueue' = RemoveSeqMember(finishedQueue, worker)
    /\ effectUsed' = IF worker \in EffectfulWorkers
                         THEN effectUsed + 1
                         ELSE effectUsed
    /\ UNCHANGED <<permitOwners, waitQueue, ingressOpen>>

DrainEffect ==
    /\ effectUsed > 0
    /\ effectUsed' = effectUsed - 1
    /\ UNCHANGED <<permitOwners, waitQueue, finishedQueue, phase, ingressOpen>>

\* Completion-ingress closure is absorbing and orthogonal. It retires one
\* receive arm but cannot disable effect drain, settlement or permit progress.
DisconnectIngress ==
    /\ ingressOpen
    /\ ingressOpen' = FALSE
    /\ UNCHANGED <<permitOwners, waitQueue, finishedQueue, phase, effectUsed>>

ResetWorker(worker) ==
    /\ worker \in Workers
    /\ phase[worker] = "settled"
    /\ phase' = [phase EXCEPT ![worker] = "idle"]
    /\ UNCHANGED <<permitOwners, waitQueue, finishedQueue, effectUsed, ingressOpen>>

Next ==
    \/ \E worker \in Workers : AcquireWorker(worker)
    \/ \E request \in Directs : QueueDirect(request)
    \/ GrantHead
    \/ \E worker \in Workers : FinishWorker(worker)
    \/ \E request \in Directs : ReleaseDirect(request)
    \/ \E worker \in Workers : SettleFinished(worker)
    \/ DrainEffect
    \/ DisconnectIngress
    \/ \E worker \in Workers : ResetWorker(worker)

TypeOK ==
    /\ permitOwners \subseteq Workers \cup Directs
    /\ Cardinality(permitOwners) <= PermitCapacity
    /\ waitQueue \in Seq(Directs)
    /\ Len(waitQueue) = Cardinality(SeqRange(waitQueue))
    /\ finishedQueue \in Seq(Workers)
    /\ Len(finishedQueue) = Cardinality(SeqRange(finishedQueue))
    /\ permitOwners \cap SeqRange(waitQueue) = {}
    /\ phase \in [Workers -> Phases]
    /\ effectUsed \in 0..EffectCapacity
    /\ ingressOpen \in BOOLEAN
    /\ \A worker \in Workers :
           (phase[worker] = "executing") <=> (worker \in permitOwners)
    /\ \A worker \in Workers :
           phase[worker] = "finished" => worker \notin permitOwners
    /\ \A worker \in Workers :
           (phase[worker] = "finished") <=> (worker \in SeqRange(finishedQueue))

WorkerSlotBound == Cardinality(Outstanding) <= WorkerCapacity

PermitCapacityBound == Cardinality(permitOwners) <= PermitCapacity

FinishedDoesNotRetainPermit ==
    \A worker \in Workers : phase[worker] = "finished" => worker \notin permitOwners

FinishedEventuallySettles ==
    \A worker \in Workers :
        (phase[worker] = "finished") ~> (phase[worker] = "settled")

QueuedDirectEventuallyOwnsPermit ==
    \A request \in Directs :
        (request \in SeqRange(waitQueue)) ~> (request \in permitOwners)

\* This deliberately false invariant is used by the reachability configuration
\* to prove the exact effect-blocked/direct-handoff state is not vacuous.
NoEffectBlockedDirectHandoff ==
    ~\E worker \in Workers :
        /\ phase[worker] = "finished"
        /\ effectUsed = EffectCapacity
        /\ permitOwners \cap Directs # {}

Spec ==
    /\ Init
    /\ [][Next]_vars
    /\ WF_vars(GrantHead)
    /\ WF_vars(DrainEffect)
    /\ \A worker \in Workers : WF_vars(FinishWorker(worker))
    /\ \A worker \in Workers : WF_vars(SettleFinished(worker))
    /\ \A request \in Directs : WF_vars(ReleaseDirect(request))

=============================================================================
