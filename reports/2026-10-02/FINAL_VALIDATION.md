# Этап 9: финальная проверка (2026-10-02, архив)

Этап 9 завершён 2026-10-02. Последние выбранные app-пакеты:
nice26.10.0-02114526 и XIAO26.10.0-02114532; подписанные обновления загрузчика,
UF2 recovery, сохранение настроек и Mac/Android rendered HID подтверждены на обеих
платах. Разделы ниже различают точные образы и область повторной проверки.
Исторические сбои сохранены. Production/release qualification в цикл не входит.

Этот отчёт относится только к образам этапа 9. Результаты обновления зависимостей
от 2026-10-03: [DEPENDENCY_HIL_NICE.md](../2026-10-03/DEPENDENCY_HIL_NICE.md). Промежуточные
`OPEN`, `pending` и указания `NEXT` в журнале ниже описывают состояние на момент
записи; они не являются актуальным списком задач.

## Последняя оптимизация GET_INFO и итоговое решение

После полной аппаратной матрицы изменилось только кодирование публичного digest
в GET_INFO: 32 вызова общего форматтера заменены двумя hex-символами на байт.
Референсный тест сравнивает все значения0..255 со стандартным `{byte:02x}`.
Финальный gate143 Rust /92 Python /17 UI, обе ARM app/boot/updater и installer,
fmt/Clippy/vendor/signature/layout/protocol passed. На обеих платах повторены
затронутые проверки: точный app/hash и ответ GET_INFO,1000 запросов/команду/фазу,
integrity4096, raw24, сохранение durable fields и BLEready после обычных updates.

| Board | App / image SHA256 | GET_INFO 04a → final before / after stress, p50ms |
| --- | --- | --- |
| nice!nano | 26.10.0-02114526 / 88ac82bbc12537edd085c96f59a8812eccf7f4f8ce9472f765462edfcec61c14 | .984 → .793 / .767 |
| XIAO | 26.10.0-02114532 / b5181f7b046c53d801ebde50312fdad7dc50b2e5e1a14da6955139f79054e5e5 | .980 → .765 / .744 |

В этих сериях GET_INFO быстрее04a примерно на20–24%. Причина улучшения —
устранение повторного форматирования digest; это не доказательство механизма
всех прежних задержек. Обёртка idle_progress и числовое форматирование версии
протокола не дали выигрыша в отдельных A/B и сохранены в рабочем коде.

PING/STATE не показывают постоянного направления изменения на двух платах:
после stress final/04a p50ms nice PING .243/.298, STATE .704/.618;
XIAO PING .288/.253, STATE .612/.654. Повтор одного и того же guarded-образа
показал STATE .722→.622ms, а один into-образ до/после stress PING .250→.301ms.
Остаточные дельты укладываются в наблюдавшуюся вариативность повторных фаз;
по этим последовательным сериям нельзя отделить влияние хоста, радио и фазы USB
или обещать отсутствие любого небольшого изменения latency. Обнаруженные
систематические затраты whole-capacity parser copies и GET_INFO formatting
устранены; отдельных незавершённых решений по производительности не осталось.

Длинные BLE reconnect tails существуют и на04a, и на текущем runtime, включая
awake Android. Все итоговые переходы прошли60s. Exclusive root cause tails не
установлена; её не выдаём за ошибку нового образа или доказанное влияние экрана.
USB packet-loss воспроизведён на08 и исправлен минимальным HAL patch на обеих
платах (4096 stress responses); это не packet-level доказательство причины каждого
исторического timeout.

**Граница доказательств:** actual782 на Mac/Android, events, пять BLEциклов и
физический coldcycle принадлежат exact into-образам02101044/02101050 из таблицы
ниже. Их вручную не переписывали как проверки новых SHA. После read-only digest
patch HID/BLE/storage/boot код не изменялся (28 source hashes checked); повторены
полный software gate и затронутый USB/metadata путь. Флаги rendered_verified
остаются только в оригинальных отчётах соответствующих образов.

Последние frozen packages/reports: `09/usb-fast-info/{board}/`;
проверенный сводный индекс `09/final-validation-summary.json`;
финальный gate `09/usb-fast-info/quality.log`. Отдельные диагностические и
отвергнутые варианты сохранены рядом.

## Полная аппаратная матрица runtime с буфером вызывающей задачи

Аппаратная матрица ниже завершена на этих точных образах до последнего
read-only digest patch. Прежние результаты и неудачи ниже остаются
историческими доказательствами, а не результатами новых образов.

| Проверка | nice!nano ECA27894EBB268AC | XIAO ED9F6FD799C4C309 |
| --- | --- | --- |
| Frozen app | 26.10.0-02101044 | 26.10.0-02101050 |
| Patched bootloader | 0.3.0-20261001233651 | 0.3.0-20261001233654 |
| USB integrity / raw / events | 4096 responses / 24 / passed | 4096 responses / 24 / passed |
| Actual rendered Mac / Android | 782 / 782, exact | 782 / 782, exact |
| Awake BLE | 10 transitions passed, Mac max41.145s / Android max6.719s | 10 transitions passed, Mac max23.944s / Android max5.887s |
| Signed boot update / UF2 refusal-recovery | exact partition / all six passed | exact partition / all six passed |
| Cold persistence on new app | passed, exact image/settings/both bonds/Android ready | passed, exact image/settings/both bonds/Android ready |
| Final software gate | 142 Rust / 92 Python / 17 UI; both ARM variants | same gate |

Bootloader source unchanged by the caller-buffer optimization: its signed
replacement and six-case qualification remain the proven packages. Final app
source/UF2 snapshots are `09/usb-final-into/`; per-board app/USB/rendered/BLE/cold
reports are `09/{board}/final-into-candidate/`. The validated aggregate is
`09/final-into-summary.json`.

Caller-owned parsing removes two whole-capacity 516-byte copies identified in
ARM disassembly. Matched RTC profiler measurements fell from68.5–69.1us to
13.4–14.4us per short command; these include interrupt/executor delays and RTC
quantization. A trace-enabled A/B reduced PING p50 .301→.254ms, with restored
candidate .321ms; host drift prevents equating every RTT difference to CPU time.
Idle-guard removal did not improve INFO (.990/1.008/.949ms guarded/direct/guarded).
The guard remains for watchdog correctness during legitimate unpolled bulk IN.

Into raw app nice292988/XIAO292948 bytes; signed envelopes293244/293204 fit
974848. Boot47664 fits48128. Static appRAM54276 vs previous55332, boot18968,
out of245760 with16KiB reserved. This reserve is not measured stack headroom.
Latest fast-info raw app nice292964/XIAO292924 bytes; signed293220/293180.
No numeric build speedup is claimed from single cold-own-crate/warm pairs with
cached dependencies. Fixed-version into own-crate build6.030s/warm.114s gave
identical frozen ELF (`09/final-into-build-time.json`); latest fast-info timing
is recorded separately in `09/final-fast-info-build-time.json`.

<details>
<summary>Исторические кандидаты, сбои и журнал диагностики</summary>

## Предыдущий кандидат с возвратом OwnedFrame (история)

| Проверка | nice!nano | XIAO |
| --- | --- | --- |
| Frozen app | 26.10.0-01232909 | 26.10.0-01232917, installed/exact verified |
| Driver-patched boot | 0.3.0-20261001233651, exact partition verified | 0.3.0-20261001233654, exact partition verified |
| USB integrity / raw / events | 4096 responses / 24 / passed | 4096 responses / 24 / passed |
| Actual rendered Mac / Android | 782 / 782, exact | 782 / 782, exact |
| Awake BLE A/B | 30 transitions passed, tails on both images | candidate 10 transitions passed; Mac max16.99s / Android max6.08s |
| New boot UF2 refusal/recovery | all six passed, settings preserved | all six passed, settings preserved |
| Cold persistence | passed, fresh confirmed 5s power cycle; exact app/settings/Android ready | passed, fresh confirmed 5s power cycle; exact app/settings/Android ready |
| Final software gate | 141 Rust / 92 Python / 17 UI; both ARM variants | same gate |

## Историческая матрица 08/07

| Проверка | XIAO ED9F6FD799C4C309 | nice!nano ECA27894EBB268AC |
| --- | --- | --- |
| Проверенная app этапа 08 / boot этапа 07 | exact identity passed | exact app identity passed; boot из прежнего checkpoint |
| UF2 отказ/восстановление, сохранение настроек | 6 passed | 6 passed |
| Raw USB framing | 24 passed | первый запуск: 17 passed, timeout maximum-plus-next; после USB reconnect 24 passed |
| USB events/overflow/snapshot/live BLE controls | passed | passed |
| Mac contract | 2 passed | 2 passed |
| Mac фактический длинный ввод | 798 символов, точное совпадение | 798 символов, точное совпадение |
| Android фактический длинный ввод | повтор: 782 символа, точное совпадение | 782 символа, точное совпадение |

Каждая проверка ввода выполнена после нового подтверждения фокуса. Enter не
отправлялся. Фактические копии и SHA сохранены в `mac-long-hid.json` и
`android-long-hid.json` соответствующей платы. Baseline `Pager test 123` содержит
14 символов; прежняя оценка 13 была ошибочной. Mac включает 16 символов
contract-prefix, поэтому его итог 798, Android — 782.

## Измерения

| USB команда, 100 samples | XIAO p50 / p95, мс (repeat) | nice p50 / p95, мс |
| --- | --- | --- |
| PING | 0.413 / 0.468 | 0.402 / 0.444 |
| GET_INFO | 1.139 / 1.192 | 1.126 / 1.194 |
| GET_STATE | 0.804 / 0.873 | 0.804 / 0.865 |

Для XIAO сравнимый paired fixture этапа 04a: 0.231/0.281, 0.878/0.970,
0.592/0.657 мс соответственно. Рост примерно 0.2 мс пока не объяснён;
отсутствие регрессии не заявляется. Совпадение state header не обеспечивает
одинаковые условия хоста или радио.

Nice BLE, пять циклов, deadline 45 с: Mac 5.025–5.369 с, Android
15.873–44.896 с. Отдельный переход перед Android-вводом занял 50.390 с при
deadline 60 с. XIAO ранее показала Mac tails 25–28 с. Публичное состояние
Advertising и лог BLE:ADVERTISING предшествуют завершению advertiser enable;
по ним нельзя отделить настройку контроллера от ожидания подключения хоста.

## Сбои, открытые на момент этой записи (история)

1. XIAO Android: baseline+long1 получили ACK; long2 timeout 12 с, long3 не
   отправлен. GET_INFO новых клиентов также timeout, CDC heartbeat работает.
   Фактические 270 символов содержали три a→A после пунктуации; после отключения
   автокапитализации и USB reconnect повтор дал точные 782 символа.
   Автокапитализация согласуется с отличиями текста, но не объясняет USB-сбой.
2. Nice raw USB: после 17 случаев maximum-plus-next не получил ответа;
   GET_INFO новых клиентов timeout, CDC heartbeat работает. После USB reconnect
   30 целевых случаев (payload 257/511/512 + PING) и полный raw repeat прошли.
   Связь с XIAO-сбоем не доказана; прошивка не изменялась.
3. Задержки BLE и рост USB latency требуют диагностики или контролируемого A/B.
   Уменьшение существующих settling delays без доказательств не выполнялось.

Первичная миграция заводской XIAO Sense A222C62566851775 с stock 0.6.1 и
последующее signed chip-bound bootloader update/ordinary restore/raw24 прошли.
Артефакты: `dist/checkpoints/09/factory-A222C62566851775/`.

Полные журналы и измерения: `dist/checkpoints/09/{xiao-nrf52840,nice-nano-v2}/`.
Эта историческая запись предшествует завершённой итоговой матрице выше.

## Диагностика успешного advertiser enable

В исходники добавлен лог `BLE:ADV_ENABLED:slot=…:uptime_ms=…:setup_ms=…`
после успешного возврата API enable, до ожидания accept. Полный gate прошёл;
отдельные подписанные пакеты сохранены в `dist/checkpoints/09/advertiser-diagnostic/`.
Ключ и layout совпадают с этапом 08. Пользователь разрешил временную установку
на nice: exact identity и настройки проверены, десять BLE-переключений прошли.
Проверенная 08 восстановлена, exact identity/настройки/Mac HID ready подтверждены.
Результаты основной матрицы выше относятся к 08; диагностический пакет не прошёл
полную финальную матрицу.

В диагностической трассе Mac tail 26.989 с: successful enable через 3.113 с,
повторное enable через 13.114 с, Connecting 25.309 с, Connected 25.522 с,
HID ready 26.986 с. Оба enable setup <1 мс. В этой выборке задержка находится
после успешного enable, до принятия BLE-соединения. Причина позднего соединения
не определена; данные не доказывают вину Mac или отсутствие проблемы радио.
Android tail 50 с с новой меткой пока не воспроизведён.

## Уточнение состояния Android

Пользователь сообщил: «Экран скорее всего был погашен» во время прежних длинных
переключений. Это ретроспективная оценка, а не запись состояния каждого замера.
Сравнение на той же 08 выполнено после подтверждения готовности телефона:
пять переходов на Android при включённом экране — 5.352–5.978 с, median 5.557 с;
раньше — 15.873–44.896 с, median 33.749 с. Это поддерживает зависимость от
состояния телефона; ретроспективная неопределённость прежнего состояния и
последовательное проведение серий не позволяют объявить единственную причину.
Mac в новой серии — 4.922–16.051 с; его хвосты рассматриваются отдельно. Этот фактор не объясняет Mac tail или USB-сбои.

## Контролируемое USB A/B на nice

Проверенная 08 → ранее проверенная 04a → восстановленная 08, по 100 samples
на команду, тот же USB-путь/ключ/layout/schema 7, Mac slot 1 HID ready.
Точный образ и совпадение durable settings проверены во всех трёх фазах.

| Команда p50, мс | 08 до | 04a | 08 после |
| --- | --- | --- | --- |
| PING | 0.414 | 0.260 | 0.428 |
| GET_INFO | 1.162 | 0.950 | 1.134 |
| GET_STATE | 0.837 | 0.584 | 0.805 |

Различие повторяется на одном стенде и связано с выбором образа в этой серии.
Конкретный участок кода ещё не найден. Прежняя гипотеза только об изменении
условий хоста недостаточна. Заявления об отсутствии USB latency regression нет.
Данные: `usb-controlled-ab.json`, summary и журнал в checkpoint nice 09.

## Подготовленная USB-диагностика

Сборка с `PAGER_USB_TRACE=1` добавляет CDC `USB:PROGRESS` каждые 10 с:
phase 1 — select/read/event wait, 2 — разбор принятых байтов, 3 — bulk IN write,
4 — bulk IN frame submitted. Выводит число пакетов/байтов и последний public
request ID/opcode, без payload или key material. Пакеты явно помечены
`usb_trace: true`; release с этим env запрещён. Без флага трасса отключена.

Полный host/ARM gate прошёл; отдельные пакеты сохранены в
`dist/checkpoints/09/usb-diagnostic/`. Nice диагностический пакет временно установлен с разрешения пользователя:
восемь connected raw-runs и четыре во время переключений BLE прошли, 288 случаев.
CDC progress работает; зависание не воспроизведено, исправление не заявляется.
08 восстановлена, exact identity/настройки/Mac HID ready подтверждены.
XIAO диагностический пакет временно установлен после нового разрешения:
exact identity и сохранение настроек подтверждены, Android slot 2 HID ready.
Повтор Android-ввода с CDC-трассой прошёл: 782 символа совпали посимвольно,
USB продолжил отвечать; зависание не воспроизведено. 08 восстановлена с
проверкой exact identity, настроек и Android HID ready. Тестовый helper получил уникальный
счётчик каталогов и stderr Git после одного gate-сбоя с недиагностированным
Git subprocess; причина первого сбоя не установлена, повтор gate прошёл.

## Кандидат уменьшения работы парсера

Дополнительное XIAO A/B 08→05b→08 на Android slot 2 подтвердило повышенную
latency уже на 05b: p50 PING 0.399/0.451/0.424 мс, GET_INFO
1.111/1.383/1.129 мс, GET_STATE 0.760/0.994/0.807 мс. 08 восстановлена.
Это сужает поиск, но не доказывает конкретную причину.

Подготовлен `OwnedFrame` с bounded heapless Vec вместо заполненного нулями
512-байтового массива: инициализируются только bytes действительного payload.
Ownership сохраняется после следующего кадра и reset, включая пустой/max payload;
новый тест покрывает этот контракт. Полный gate 140 Rust/92 Python/17 UI и обе
ARM-сборки прошли. Пакеты сохранены в `dist/checkpoints/09/owned-payload/`.
XIAO прошла ограниченный USB HIL ниже; полной квалификации кандидата пока нет.

### XIAO: A/B кандидата OwnedFrame

После разрешения пользователя выполнено 08 → кандидат → 08, по 100 samples
на команду, Android slot 2 HID ready, без HID-ввода. Exact identity, ключ/layout
и сохранение durable settings проверены во всех фазах.

| Команда p50, мс | 08 до | Кандидат | 08 после |
| --- | --- | --- | --- |
| PING | 0.402 | 0.416 | 0.423 |
| GET_INFO | 1.115 | 1.043 | 1.160 |
| GET_STATE | 0.800 | 0.713 | 0.803 |

GET_INFO/GET_STATE быстрее кандидата в этой серии; PING не показывает явного
улучшения. Это частичный результат одного A/B, не полное объяснение регрессии.
Все 24 raw framing cases прошли, включая maximum-plus-next, durable state
сохранён. Qualified 08 восстановлена: exact version 26.10.0-01130342,
SHA 8733986f6082722f681a207e3d2217a9cb9d187439d73beeed0d719887ffbf07;
Android slot 2 connected/HID ready, оба bond и настройки совпали. Events и
rendered HID на кандидате в этой серии не проверялись; nice кандидат затем проверен ниже.
Прежние USB stalls не воспроизведены и не объявляются исправленными.
Данные: `09/xiao-nrf52840/usb-controlled-ab-owned-payload{,-summary}.json` и log.

### nice: A/B кандидата OwnedFrame

После нового + выполнено 08 → кандидат → 08, 100 samples/command,
Mac slot 1 connected/HID ready, exact identities/durable settings совпали.

| Команда p50, мс | 08 до | Кандидат | 08 после |
| --- | --- | --- | --- |
| PING | 0.400 | 0.387 | 0.446 |
| GET_INFO | 1.127 | 1.049 | 1.170 |
| GET_STATE | 0.810 | 0.707 | 0.788 |

Все 24 raw framing cases прошли, настройки сохранены, HID не отправлялся.
Qualified08 восстановлена за 16.79 с: exact version 26.10.0-01130335,
SHA6372544146ae181e34332ba5f7ce42ac9b7f51a60b94863c521d9ddc594627f8,
оба bond и Mac slot 1 connected/HID ready проверены. Данные в
`09/nice-nano-v2/usb-controlled-ab-owned-payload{,-summary}.json` и log.
Частичное улучшение GET_INFO/GET_STATE повторилось на двух платах; эффект PING
неодинаков, заметен дрейф между сериями. Полного возврата к 04a и исправления
зависаний эти данные не доказывают. Candidate events/rendered HID ещё не проверены.

### 2026-10-02 — nice candidate events passed; HID/USB stall reproduced

Fresh + authorized temporary candidate and Mac safe focus. Nice candidate exact
26.10.0-01225238 installed16.54s, settings matched. Event interleave/overflow/
gap/snapshot/live USB during BLE controls passed; fixture returned Macslot1ready.
HID baseline14 ACK .344s; long1 256bytes timed out, long2/3 not sent.
USB GET_INFO during qualified08 restore also timed out; restore NOT performed.
Only nice ECA27894EBB268AC remains on candidate, USB unresponsive. Physical
USB reconnect requires fresh user +, then exactcandidatepreflight/restore08,
settings/bonds/Macready verification. No retry HID without freshfocus. Actual
rendered text pending. Evidence09/nice-nano-v2/owned-payload-qualification/
qualification.json, mac-long-hid.json and parent owned-payload-qualification.log.
Candidate DOES NOT fix original stall class; no full qualification claim.

### 2026-10-02 — nice physical recovery after candidate HID stall

User + confirmed USB reconnect; candidate USB responded with exact identity.
First recovery helper assertion compared decoded tuple fields with JSON lists;
no flash or HID occurred before assertion. Canonical JSON comparison corrected;
settings match. Qualified08 restored16.59s, exact26.10.0-01130335/
SHA6372544146ae181e34332ba5f7ce42ac9b7f51a60b94863c521d9ddc594627f8,
both bonds/settings preserved, Macslot1connected/HIDready verified, exit0.
Only nice connected, no USB owner active. CDC after stall recorded heartbeat
10–50s despite bulk command timeouts. User actualcopy “Зфпук еуые 123” corresponds
to baseline keys with Russian layout; English rendering not verified, no long
block present in supplied copy. USB stall remains a separate observed failure.
Evidence owned-payload-qualification/{physical-recovery,qualification,mac-long-hid}.json,
recovery.log. Candidate fails full HID qualification; stage9 remains OPEN.

### 2026-10-02 — USB packet-loss mechanism and reproducible non-HID stress

Owned-payload trace26.10.0-01231113: events passed, baseline14+3x256 ACK,
actual user Mac copy exact782 verified; qualified08 restore verified. The earlier
candidate stall remains a recorded failure, not erased by this repeat.

Identified embassy-nrf0.11.0 read_dma double-arm: after ENDEPOUT it writes
SIZE.EPOUT, potentially discarding an intervening unread packet. Primary refs:
https://github.com/embassy-rs/embassy/pull/6688
https://devzone.nordicsemi.com/f/nordic-q-a/35362/usb-bulk-out-hardware-bug
These document the driver mechanism; linkage to each prior Pager timeout is a
hypothesis pending patched A/B, not a claim of captured packet-level proof.

New tools/test_usb_integrity.py sends deterministic synthetic maximum frames
(unsupported command)+PING in sustained 8720-byte batches with concurrent IN
reader. No HID/no settings changes. On exactnice08: four batches passed (128
responses), batch4 returned malformed-frame ERROR request0 from valid host frames.
Report/log usb-integrity-unpatched08 saved. Reselecting USB configuration followed
by draining177 stale response bytes recovered USB without physical changes;
exact08/settings/bonds/Macslot1ready preserved. Fresh client first encountered
oldresponse100159 after reconfiguration; recorded as stale response, not a timeout.

Minimal HAL patch vendored from exactregistry0.11.0/commit3861d3088da30d40c777dc05d282352e68ec5511:
remove postDMA SIZE write, retaininitialenable. App and bootloader use same patch.
Licenses retained, inventory hashes declared, registry lint compatibility allows
specific pre-existing Clippy lints plus equivalent cfg simplification; formatting
recorded separately. Erratum199 is independent and excluded from this minimal
packet-loss patch. Fullgate pending, patched packages NOT installed. Currentnice08,
noUSBowner. NEXT freezepassedpackages; controlledpatchedintegrity/raw/events/latency
thenrenderedHID/bothboards and signedbootloader qualification before closingstage9.

### 2026-10-02 — driver fix HIL, chunk parser candidate, remaining comparison

Nice driver-only patch26.10.0-01232234 passed 128 sustained batches/4096 responses,
raw24/events and restored08 exact/settings preserved. Original08 integrity failed
batch4 after4 successful batches. This is controlled evidence for the packet-loss
fix; individual earlier stalls were not captured packet-for-packet.

Chunk parser appends available slices and drains at capacity; tests cover empty,
small/max payloads, max+next packet, fragmentation sizes1/2/16/64/127/528/900 and
CRC/garbage resynchronization. Owned payload remains bounded and independent.
Fullgate141Rust/92Python/17UI/bothARMpassed; final-candidate snapshots same source
a687cf896302a2683e1d10dda5d2dc972b3d3769b3de043e70551e5758cb06e6 for both apps/boots.
Nice app26.10.0-01232909 SHA6deca23f2b583aedbf2d48fa02aa41d59327c604010858cb09fb0d21a033fb75,
XIAO app26.10.0-01232917 SHA3aafcd5dd2216f1bee928dacb16235739f669bf9c4b9fbbe2a38e7460299610b.
Fresh + allowed nice install and remaining on candidate for Android qualification.
Nice08→04a→candidate USB100: p50ms PING .401/.234/.322(afterstress .299),
GET_INFO1.124/.888/1.047(after1.000), GET_STATE .791/.601/.671(after.659).
128batches/4096responses, raw24/events passed; Macusercopyexact782verified.
Androidawake5cycles: Android5.325–6.289s median5.702; Mac4.932–24.938s median17.916.
AndroidHID all4ACK/782expected, actualcopy pending. On-device NICEcandidate,
Androidslot2connected/HIDready, settings retained; no activeUSBowner.

Remaining USBdelta vs04a .06–.11ms is not yet dismissed. Additional inlining of
owned-frame return prepared to avoid maximum-size moves across opt-z call boundary;
fullgate passed, frozen09/usb-inline-candidate packages NOT installed.
ControlledawakeBLEcandidate→04a→candidate5cycles each prepared, awaitingphone+
to distinguish image-dependent Mac tail from common reconnect behavior.
Chip-bound signed bootupdaters forboth serials prepared/preflightpassed in
09/usb-final-candidate/{board}/bound, with exactembeddedimage/restore checks;
bootloader NOT installed, currentboot remains07. Do not powercycle during updater.
NewdriverbootUF2recovery/coldpersistence, XIAOcandidate USB/HID/BLE, and final
latency/source decision remain required before stage9completion.


### 2026-10-02 — nice candidate rendered Android and awake BLE controlled A/B

Candidate 26.10.0-01232909 Android actual copy exactly matches all 782 expected
characters (baseline + three 256-character jobs), no Enter. Mac actual 782 also
verified. Awake Android was explicitly confirmed before five cycles per image:
candidate → qualified04a → same candidate. Exact images/settings verified and
candidate restored. All 30 transitions passed the 60-second deadline; no HID sent.
Mac median seconds: 5.890 / 5.329 / 5.364; Android: 5.554 / 27.382 / 13.492.
Android maxima: 5.885 / 53.507 / 56.495. Long tails occur on both old04a and
candidate with screen confirmed on; this series does not establish a candidate
BLE regression or an exclusive screen/host/radio cause. Successful advertiser
setup was previously measured below 1 ms; reconnect waiting remains variable.
Evidence: dist/checkpoints/09/nice-nano-v2/final-candidate/ble-controlled-ab.json
and log; android-long-hid.json records actual rendering. Stage9 remains open for
USB latency decision, updated bootloader and XIAO final qualification.


### 2026-10-02 — parser inlining A/B rejected

Fresh approved USB-only chunk→inline→chunk on nice preserved settings and restored
26.10.0-01232909. 100 samples/command, p50 ms before/inline/restored:
PING .281/.317/.319, INFO 1.012/1.059/1.063, STATE .644/.717/.719.
Inline passed 4096 stress responses and raw24; inline and restored are essentially
identical in this run, with drift from the initial phase. No measured inlining
benefit; removed inline attributes/comment from source, retaining tested chunk
parser and HAL fix. Frozen rejected packages/reports preserved as evidence.
Evidence09/nice-nano-v2/usb-controlled-ab-inline.json and usb-integrity-inline.json.


### 2026-10-02 — final source gate after rejecting inlining

make quality passed (dist/checkpoints/09/final-quality.log): 141 Rust tests,
92 Python tests, 17 UI behavioral tests, vendor provenance/inventory, formatting,
Clippy and both ARM app/bootloader/updater builds, XIAO installer, signatures,
layout/protocol checks. Functional app source is the frozen chunkcandidate again;
new build outputs remain separate from the immutable HIL-qualified package.
Nice driver-patched bootloader 0.3.0-20261001233651 installed via approved signed
chip-bound updater26.10.0-01233653. Exact boot partition hash and ordinary restored
app01232909 verified; durable settings preserved. UF2 refusal/recovery still running,
so final boot qualification/cold persistence are not yet claimed.


### 2026-10-02 — nice patched bootloader UF2 qualification complete

Signed chip-bound update26.10.0-01233653 installed boot0.3.0-20261001233651;
exact partition SHA2cf3c8cdc85a76d18c67bfe53bedf94b6c1b5b99aad633275568f981477ed549
verified. App01232909 restored. All six UF2 trials passed: invalid signature,
invalid flags, storage address, omitted last block, image digest, reordered plus
exact duplicate valid blocks. Durable settings preserved after each restore;
Androidslot2 connected/HIDready at the end. Reports final-candidate/boot-update,
boot-qualification, uf2-new-boot JSON/log. Cold power persistence fresh user action
requested; not yet verified. New boot XIAO remains uninstalled.


### Throughput/resources comparison for the current frozen candidate

Nice actual-rendered candidate: Mac three 256-character jobs 6.2716/6.2714/6.2805s
(40.76–40.82 chars/s), Android 6.2693/6.2697/6.2696s (40.83 chars/s).
The 14-character 04a Mac baseline was 344.70ms (40.62 chars/s); candidate baseline
343.76ms (40.73 chars/s). The unchanged 12ms key report cadence gives consistent
measured throughput; short/long jobs are not interchangeable benchmark workloads.
Frozen raw app bytes nice292924 vs08293060, XIAO292892 vs08292924; boot47664 bytes.
Budget-check app envelope293180/974848 and boot47664/48128; static RAM app55332,
boot18968 out of245760 with16KiB reserve. Reserve is not measured stack headroom.
Stage7 fixed-version isolated app build before27.856/.345s and after27.031/.265s
(cold own crate / warm with cached dependencies) yielded identical binaries; one
pair does not establish a speedup. No numeric final build speed improvement claim.


Final source fixed-version direct nice ARM build measured own-crate rebuild5.994s,
warm repeat0.117s, identical ELF SHA4c1bb556f80e659b0fd7348852ff6f3c6cf8b19e1d1ff3c8d54c55160b2f62ec.
Dependencies were already cached; this is not a fresh dependency build and the
single pair/changed cache state does not establish a speedup over stage7.
Evidence09/final-build-time.json/log. Frozen signed candidate was not overwritten.

</details>
