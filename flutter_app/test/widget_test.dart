import 'dart:async';
import 'dart:ui' show PointerDeviceKind;

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import 'package:lanvault_app/theme/appearance_controller.dart';
import 'package:lanvault_app/theme/motion_slide.dart';
import 'package:lanvault_app/theme/skins.dart';

import 'package:lanvault_app/bridge/lanvault_bridge.dart';
import 'package:lanvault_app/bridge/system_env_helper.dart';
import 'package:lanvault_app/bridge/window_chrome.dart';
import 'package:lanvault_app/features/backup/backup_screen.dart';
import 'package:lanvault_app/features/settings/settings_screen.dart';
import 'package:lanvault_app/features/snapshots/snapshots_screen.dart';
import 'package:lanvault_app/features/vault/vault_screen.dart';
import 'package:lanvault_app/main.dart';
import 'package:lanvault_app/theme/neu_theme.dart';
import 'package:lanvault_app/theme/neu_toast.dart';
import 'package:lanvault_app/widgets/nav_rail_item.dart';
import 'package:lanvault_app/widgets/windows_title_bar.dart';

class _FakeLanVaultFfi implements LanVaultRustFfi {
  _FakeLanVaultFfi({
    this.serverRunning = false,
    List<SyncRuleModel>? syncRules,
  }) : syncRules = syncRules ?? <SyncRuleModel>[];

  bool serverRunning;
  List<SyncRuleModel> syncRules;
  final List<(String requestId, bool accept)> pairResponses =
      <(String, bool)>[];

  @override
  Future<SystemOverviewModel> getSystemOverview() async {
    return SystemOverviewModel(
      serverRunning: serverRunning,
      serverPort: serverRunning ? 7842 : null,
      lanIp: '192.168.1.10',
      pairingPin: '123456',
      pairingQrUri: '',
      vaultUsedBytes: 0,
      vaultQuotaBytes: 100 * 1024 * 1024 * 1024,
      vaultCount: 0,
      pairedPeerCount: 0,
      onlinePeerCount: 0,
      syncRuleCount: syncRules.length,
      dedupRatio: 0.0,
      compressionRatio: 1.0,
      bitRotHealth: BitRotHealth.unknown,
      corruptedChunks: 0,
      recentActivities: const <RecentActivityModel>[],
    );
  }

  @override
  Future<List<VaultSummaryModel>> listVaults() async =>
      const <VaultSummaryModel>[];

  @override
  Future<LastServerConfig?> getLastServerConfig() async => null;

  @override
  Future<Map<String, dynamic>> runSelfDiagnostics() async => <String, dynamic>{
        'server_running': serverRunning,
        'server_port': serverRunning ? 7842 : null,
      };

  @override
  Future<List<SyncRuleModel>> listSyncRules() async => syncRules;

  @override
  Future<BackupStorageConfigModel> getBackupStorageConfig() async =>
      const BackupStorageConfigModel(primaryDir: r'D:\EveryoneNAS_Vault');

  @override
  Future<bool> getSyncPaused() async => false;

  @override
  Future<List<PairedPeerModel>> listPairedPeers() async =>
      const <PairedPeerModel>[];

  @override
  Future<List<SnapshotSummaryModel>> listSnapshots({String? vaultId}) async =>
      const <SnapshotSummaryModel>[];

  @override
  Future<List<TransferHistoryModel>> listTransferHistory({
    int limit = 50,
  }) async =>
      const <TransferHistoryModel>[];

  @override
  Future<PowerNetworkPolicyModel> getPowerNetworkPolicy() async =>
      const PowerNetworkPolicyModel(
        wifiOnly: true,
        allowedSsids: <String>[],
        requireCharging: false,
        maxKbps: 0,
      );

  @override
  Future<bool> getAutoReconnect() async => true;

  @override
  Future<RoleMode> getRoleMode() async => RoleMode.dual;

  @override
  Future<bool> getDebugMode() async => false;

  @override
  Future<bool> getBitRotAutoScrub() async => true;

  @override
  Future<List<PendingPairRequestModel>> listPendingPairRequests() async =>
      const <PendingPairRequestModel>[];

  @override
  Future<bool> respondPairRequest({
    required String requestId,
    required bool accept,
  }) async {
    pairResponses.add((requestId, accept));
    return true;
  }

  @override
  Stream<SyncProgressEvent> subscribeSyncProgress() =>
      const Stream<SyncProgressEvent>.empty();

  @override
  Stream<DiscoveredPeer> startMdnsScan() =>
      const Stream<DiscoveredPeer>.empty();

  @override
  Future<void> updateDeviceEnvironment({
    required bool onWifi,
    String? currentSsid,
    required bool isCharging,
  }) async {}

  @override
  Future<NetworkScanSnapshot> scanWindowsNetworks() async =>
      const NetworkScanSnapshot(
        hasWifiAdapter: false,
        isWiredEthernet: true,
        currentSsid: null,
        availableSsids: <String>[],
      );

  @override
  Future<void> reportClientLog({
    required String level,
    required String source,
    required String message,
    String? detail,
  }) async {}

  @override
  Future<void> clearDebugLogs() async {}

  @override
  Future<VaultSummaryModel> createVault({
    required String name,
    required String mountPath,
    required PoolMode poolMode,
    required int quotaGb,
    required bool e2eeEnabled,
    String? extraMountPath,
  }) =>
      throw UnimplementedError();

  @override
  Future<bool> deleteSyncRule(String ruleId) async => true;

  @override
  Future<void> enableAutoReconnect(bool enabled) async {}

  @override
  Future<String> exportVaultKeyBackup(String passphrase) async => '';

  @override
  Future<String?> getDebugLogPath() async => null;

  @override
  Future<List<DebugLogEntryModel>> getDebugLogs({int limit = 200}) async =>
      const <DebugLogEntryModel>[];

  @override
  Future<PairingInfoModel> getPairingInfo() async => const PairingInfoModel(
        serverRunning: false,
        lanIp: '192.168.1.10',
        port: 7842,
        pairingPin: '123456',
        pairingQrUri: '',
      );

  @override
  Future<void> importVaultKeyBackup(
      String passphrase, String backupJson) async {}

  @override
  Future<String> pairWithPin({
    required String ip,
    required int port,
    required String pin,
  }) async =>
      'peer-1';

  @override
  Future<void> prepareGracefulRestart() async {}

  @override
  Future<bool> clearWebPassword() async => true;

  @override
  Future<bool> pushClipboardFile({
    String? targetDeviceId,
    required String filePath,
  }) async =>
      true;

  @override
  Future<void> pushClipboardText({
    String? targetDeviceId,
    required String text,
  }) async {}

  @override
  Future<String> requestPairing({
    required String ip,
    required int port,
  }) async =>
      'peer-1';

  @override
  Future<RestoreSnapshotResultModel> restoreSnapshot({
    required String snapshotId,
    required String targetDir,
  }) =>
      throw UnimplementedError();

  @override
  Future<bool> revokePairedPeer(String deviceId) async => true;

  @override
  Future<bool> setBitRotAutoScrub(bool enabled) async => enabled;

  @override
  Future<BackupStorageConfigModel> setBackupStorageConfig({
    required String primaryDir,
    String? fallbackDir,
  }) async =>
      BackupStorageConfigModel(
        primaryDir: primaryDir,
        fallbackDir: fallbackDir,
      );

  @override
  Future<bool> setDebugMode(bool enabled) async => enabled;

  @override
  Future<RoleMode> setRoleMode(RoleMode mode) async => mode;

  @override
  Future<bool> setSyncPaused(bool paused) async => paused;

  @override
  Future<bool> setFastBackupMode(bool enabled) async => enabled;

  @override
  Future<String> setSyncRuleSchedule({
    required String ruleId,
    required String scheduleMode,
  }) async =>
      scheduleMode;

  @override
  Future<int> startLocalServer({
    required String vaultPath,
    required int quotaGb,
    required String webPassword,
    int? port,
  }) async =>
      port ?? 7842;

  @override
  Future<bool> toggleSyncRule({
    required String ruleId,
    required bool enabled,
  }) async =>
      enabled;

  @override
  Future<BitRotScrubResultModel> triggerBitRotScrub() async =>
      const BitRotScrubResultModel(
        scrubId: 's1',
        checkedChunks: 0,
        corruptedChunks: 0,
        healedChunks: 0,
        status: 'healthy',
      );

  @override
  Future<void> triggerIncrementalScan(String ruleId) async {}

  @override
  Future<void> updatePowerNetworkPolicy({
    required bool wifiOnly,
    required List<String> allowedSsids,
    required bool requireCharging,
    required int maxKbps,
  }) async {}

  @override
  Future<SyncRuleModel> upsertSyncRule({
    required String ruleId,
    required String localDir,
    String targetVaultId = 'default',
    bool enabled = true,
  }) async =>
      SyncRuleModel(
        ruleId: ruleId,
        localDir: localDir,
        targetVaultId: targetVaultId,
        enabled: enabled,
      );

  @override
  Future<int?> autostartLocalServer() async => serverRunning ? 7842 : null;

  @override
  Future<bool> getAutostartServer() async => serverRunning;

  @override
  Future<bool> setAutostartServer(bool enabled) async {
    serverRunning = enabled;
    return enabled;
  }

  @override
  Future<List<DeviceGroupViewModel>> listDeviceGroups() async =>
      const <DeviceGroupViewModel>[];

  @override
  Future<DeviceGroupModel> createDeviceGroup(String name) async =>
      DeviceGroupModel(
        groupId: 'grp-1',
        name: name,
        coordinatorDeviceId: 'local',
        revision: 1,
        createdAt: 0,
        updatedAt: 0,
      );

  @override
  Future<DeviceGroupModel> renameDeviceGroup({
    required String groupId,
    required String name,
  }) async =>
      DeviceGroupModel(
        groupId: groupId,
        name: name,
        coordinatorDeviceId: 'local',
        revision: 2,
        createdAt: 0,
        updatedAt: 0,
      );

  @override
  Future<void> deleteDeviceGroup(String groupId) async {}

  @override
  Future<void> addGroupMember({
    required String groupId,
    required String deviceId,
    String? role,
  }) async {}

  @override
  Future<void> removeGroupMember({
    required String groupId,
    required String deviceId,
  }) async {}

  @override
  Future<List<GroupRuleViewModel>> listGroupRules({String? groupId}) async =>
      const <GroupRuleViewModel>[];

  @override
  Future<GroupSyncRuleModel> upsertGroupRule({
    String? ruleId,
    required String groupId,
    required String name,
    required String mode,
    String? sourceDeviceId,
    required String sourceRoot,
    String? sourceRootId,
    int archiveRetentionDays = 30,
    bool wifiOnly = true,
    String? allowedSsids,
    bool requireCharging = false,
    int maxKbps = 0,
    int scheduleIntervalSecs = 0,
    bool enabled = true,
  }) async =>
      GroupSyncRuleModel(
        ruleId: ruleId ?? 'grule-1',
        groupId: groupId,
        name: name,
        mode: mode,
        sourceDeviceId: sourceDeviceId ?? 'local',
        sourceRoot: sourceRoot,
        sourceRootId: sourceRootId,
        archiveRetentionDays: archiveRetentionDays,
        wifiOnly: wifiOnly,
        allowedSsids: allowedSsids,
        requireCharging: requireCharging,
        maxKbps: maxKbps,
        scheduleIntervalSecs: scheduleIntervalSecs,
        enabled: enabled,
        revision: 1,
        groupRevision: 1,
        needsBindingConfirm: false,
        createdAt: 0,
        updatedAt: 0,
      );

  @override
  Future<void> deleteGroupRule(String ruleId) async {}

  @override
  Future<void> toggleGroupRule({
    required String ruleId,
    required bool enabled,
  }) async {}

  @override
  Future<RuleTargetBindingModel> upsertRuleTargetBinding({
    String? bindingId,
    required String ruleId,
    required String targetDeviceId,
    String? targetVaultId,
    String? mirrorRootId,
    String? targetRoot,
    bool enabled = true,
  }) async =>
      RuleTargetBindingModel(
        bindingId: bindingId ?? 'bind-1',
        ruleId: ruleId,
        groupId: 'grp-1',
        targetDeviceId: targetDeviceId,
        targetVaultId: targetVaultId,
        mirrorRootId: mirrorRootId,
        targetRoot: targetRoot,
        enabled: enabled,
        appliedRevision: 0,
        lastSourceGen: 0,
        status: 'pending',
        needsBindingConfirm: false,
        createdAt: 0,
        updatedAt: 0,
      );

  @override
  Future<void> disableRuleTargetBinding(String bindingId) async {}

  @override
  Future<MirrorRootModel> registerMirrorRoot({
    required String canonicalPath,
    String displayName = '',
  }) async =>
      MirrorRootModel(
        mirrorRootId: 'mroot-1',
        deviceId: 'local',
        canonicalRoot: canonicalPath,
        label: displayName.isEmpty ? 'Mirror' : displayName,
        createdAt: 0,
      );

  @override
  Future<List<MirrorRootModel>> listMirrorRoots() async =>
      const <MirrorRootModel>[];

  @override
  Future<void> revokeMirrorRoot(String rootId) async {}

  @override
  Future<List<MirrorPreviewModel>> previewGroupRule(String ruleId) async =>
      const <MirrorPreviewModel>[];

  @override
  Future<GroupRuleSyncOutcomeModel> triggerGroupRuleSync(String ruleId) async =>
      GroupRuleSyncOutcomeModel(
        ruleId: ruleId,
        sourceComplete: true,
        scannedEntries: 0,
        blockedReasons: const <String>[],
        volatilePaths: const <String>[],
        bindings: const <BindingSyncOutcomeModel>[],
      );

  @override
  Future<List<CatalogEntryModel>> listBindingCatalog({
    required String bindingId,
    String? afterPathKey,
    int? limit,
    bool sourceDeletedOnly = false,
  }) async =>
      const <CatalogEntryModel>[];

  @override
  Future<List<CatalogVersionModel>> listBindingVersions({
    required String bindingId,
    String? pathKey,
    int? afterArchivedAt,
    String? afterVersionId,
    int? limit,
  }) async =>
      const <CatalogVersionModel>[];

  @override
  Future<RestoreCatalogVersionResultModel> restoreCatalogVersion({
    required String bindingId,
    required String versionId,
    required String targetDir,
  }) =>
      throw UnimplementedError();

  @override
  Future<OutboxMessageViewModel> enqueueMessageText({
    required List<String> targetDeviceIds,
    required String text,
  }) async =>
      OutboxMessageViewModel(
        clientMsgId: 'msg-1',
        localSeq: 1,
        sessionKey: targetDeviceIds.join(','),
        kind: 'text',
        title: text,
        content: text,
        sizeBytes: text.length,
        overallStatus: 'sending',
        createdAt: 0,
        updatedAt: 0,
        targets: const <OutboxTargetStatusModel>[],
      );

  @override
  Future<OutboxMessageViewModel> enqueueMessageFile({
    required List<String> targetDeviceIds,
    required String filePath,
  }) async =>
      OutboxMessageViewModel(
        clientMsgId: 'msg-2',
        localSeq: 2,
        sessionKey: targetDeviceIds.join(','),
        kind: 'file',
        title: filePath,
        filePath: filePath,
        sizeBytes: 0,
        overallStatus: 'sending',
        createdAt: 0,
        updatedAt: 0,
        targets: const <OutboxTargetStatusModel>[],
      );

  @override
  Future<int> retryOutboxMessage(String clientMsgId) async => 1;

  @override
  Future<List<OutboxMessageViewModel>> listOutboxMessages({int? limit}) async =>
      const <OutboxMessageViewModel>[];
}

void main() {
  tearDown(() {
    NeuToast.dismiss();
  });

  test('LanVaultBridge.formatUserError maps backend errors to friendly Chinese',
      () {
    expect(
      LanVaultBridge.formatUserError(
        Exception('invalid input: mount path overlaps existing volume H:\\'),
      ),
      contains('重叠'),
    );
    expect(
      LanVaultBridge.formatUserError(
        StateError('peer rejected pairing request'),
      ),
      '对方已拒绝本次配对请求',
    );
    expect(
      LanVaultBridge.formatUserError(
        StateError('FFI create_vault failed: local server is not running'),
      ),
      contains('请先启动本机存储服务'),
    );
  });

  testWidgets('NeuButton disabledLook uses dark炭黑 surface and triggers onTap',
      (tester) async {
    var tapped = 0;
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLanVaultTheme(),
        home: Scaffold(
          body: Center(
            child: NeuButton(
              label: '新建存储池',
              tone: NeuButtonTone.primary,
              disabledLook: true,
              onPressed: () => tapped++,
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    final container = tester.widget<AnimatedContainer>(
      find.descendant(
        of: find.byType(NeuButton),
        matching: find.byType(AnimatedContainer),
      ),
    );
    final decoration = container.decoration as BoxDecoration;
    expect(
        decoration.color,
        NeuTheme.of(tester.element(find.byType(NeuButton).first))
            .disabledDarkSurface);
    expect(decoration.gradient, isNull);

    await tester.tap(find.text('新建存储池').first);
    await tester.pump();
    expect(tapped, 1);
  });

  testWidgets('VaultScreen gates create button when local service is stopped',
      (tester) async {
    final ffi = _FakeLanVaultFfi(serverRunning: false);
    final bridge = LanVaultBridge(rustFfi: ffi);
    addTearDown(bridge.dispose);

    await tester.pumpWidget(
      MaterialApp(
        theme: buildLanVaultTheme(),
        home: Scaffold(body: VaultScreen(bridge: bridge)),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.text('新建存储池').first);
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.text('请先在设置中确认本地存储目录'), findsOneWidget);
    expect(find.text('创建存储池'), findsNothing);
    NeuToast.dismiss();
    await tester.pump(const Duration(milliseconds: 50));
  });

  testWidgets(
      'BackupScreen mobile 360px narrow layout renders folder name on single line without overflow',
      (tester) async {
    tester.view.physicalSize = const Size(360, 760);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final ffi = _FakeLanVaultFfi(
      syncRules: const <SyncRuleModel>[
        SyncRuleModel(
          ruleId: 'r-docs',
          localDir: r'C:\Users\35383\Documents',
          targetVaultId: 'default',
          enabled: true,
        ),
      ],
    );
    final bridge = LanVaultBridge(rustFfi: ffi);
    addTearDown(bridge.dispose);

    await tester.pumpWidget(
      MaterialApp(
        theme: buildLanVaultTheme(),
        home: Scaffold(body: BackupScreen(bridge: bridge)),
      ),
    );
    await tester.pumpAndSettle();

    final docFinder = find.text('Documents');
    expect(docFinder, findsOneWidget);
    final docSize = tester.getSize(docFinder);
    expect(docSize.width, greaterThan(50));
    expect(tester.takeException(), isNull);
  });

  testWidgets('Windows shell aligns caption controls and clears previous hover',
      (tester) async {
    tester.view.physicalSize = const Size(1280, 720);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    const channel = MethodChannel('com.lanvault.app/window');
    final messenger = tester.binding.defaultBinaryMessenger;
    Rect? captionExclusion;
    messenger.setMockMethodCallHandler(channel, (call) async {
      if (call.method == 'isMaximized') return false;
      if (call.method == 'setCaptionMetrics') {
        final arguments = call.arguments as Map<Object?, Object?>;
        final exclusions = (arguments['exclusions'] as List<Object?>)
            .whereType<Map<Object?, Object?>>()
            .map(
              (exclusion) => Rect.fromLTRB(
                exclusion['left'] as double,
                exclusion['top'] as double,
                exclusion['right'] as double,
                exclusion['bottom'] as double,
              ),
            )
            .toList();
        captionExclusion = exclusions.reduce(
          (widest, candidate) =>
              candidate.right > widest.right ? candidate : widest,
        );
      }
      return null;
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));

    final bridge = LanVaultBridge(rustFfi: _FakeLanVaultFfi());
    addTearDown(bridge.dispose);
    await tester.pumpWidget(LanVaultApp(bridge: bridge));
    await tester.pumpAndSettle();

    final titleBar = find.byType(WindowsTitleBar);
    expect(
      find.descendant(of: titleBar, matching: find.byType(BrandLogoFull)),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: titleBar,
        matching: find.text('v${LanVaultBridge.appVersion}'),
      ),
      findsNothing,
    );
    final close = find.byTooltip('关闭');
    expect(tester.getTopRight(close).dx, 1268);
    expect(captionExclusion?.right, greaterThanOrEqualTo(1268));

    final navItems = find.byType(NavRailItem);
    final devices = navItems.at(1);
    final backup = navItems.at(2);
    Container navSurface(Finder item) => tester.widget<Container>(
          find
              .descendant(
                of: item,
                matching: find.byWidgetPredicate(
                  (widget) =>
                      widget is Container && widget.decoration is BoxDecoration,
                ),
              )
              .first,
        );
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: tester.getCenter(devices));
    await tester.pump();
    expect(
      (navSurface(devices).decoration as BoxDecoration).color,
      NeuTheme.of(tester.element(find.byType(NavRailItem).first)).insetSurface,
    );

    await mouse.moveTo(tester.getCenter(backup));
    await tester.pump();
    expect((navSurface(devices).decoration as BoxDecoration).color, isNull);
    expect(
      (navSurface(backup).decoration as BoxDecoration).color,
      NeuTheme.of(tester.element(find.byType(NavRailItem).first)).insetSurface,
    );
    await mouse.removePointer();
    await tester.pump();
  }, skip: !WindowChrome.isSupported);

  testWidgets('settings uses shared rail, inline scrub, and subdued dividers',
      (tester) async {
    tester.view.physicalSize = const Size(1280, 720);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final bridge = LanVaultBridge(rustFfi: _FakeLanVaultFfi());
    addTearDown(bridge.dispose);
    await tester.pumpWidget(LanVaultApp(bridge: bridge));
    await tester.pumpAndSettle();

    await tester.tap(find.text('设置').first);
    await tester.pumpAndSettle();
    expect(
      find.descendant(
        of: find.byType(SettingsScreen),
        matching: find.byType(NavRailItem),
      ),
      findsNWidgets(6),
    );

    await tester.tap(find.text('Web 访问与安全').first);
    await tester.pumpAndSettle();
    final scrubButton = find.widgetWithText(NeuButton, '立即校验');
    expect(scrubButton, findsOneWidget);
    expect(
      find.descendant(
        of: find
            .ancestor(
              of: find.text('数据完整性定期检查'),
              matching: find.byType(Row),
            )
            .last,
        matching: scrubButton,
      ),
      findsOneWidget,
    );
    final integrityRow = find
        .ancestor(
          of: find.text('数据完整性定期检查'),
          matching: find.byType(Row),
        )
        .last;
    expect(
      find.descendant(of: integrityRow, matching: scrubButton),
      findsOneWidget,
    );
    expect(
      find.descendant(of: integrityRow, matching: find.byType(NeuSwitch)),
      findsWidgets,
    );
    final divider = tester.widget<Divider>(
      find
          .descendant(
            of: find.byType(SettingsScreen),
            matching: find.byType(Divider),
          )
          .first,
    );
    expect(
      divider.color,
      NeuTheme.of(tester.element(find.byType(SettingsScreen))).dividerSubtle,
    );
  });

  testWidgets('settings security row fits compact 360px layout',
      (tester) async {
    tester.view.physicalSize = const Size(360, 760);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final bridge = LanVaultBridge(rustFfi: _FakeLanVaultFfi());
    addTearDown(bridge.dispose);
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLanVaultTheme(),
        home: Scaffold(body: SettingsScreen(bridge: bridge)),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('数据完整性定期检查'), findsOneWidget);
    expect(find.widgetWithText(NeuButton, '立即校验'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('LanVaultShell shows incoming pairing dialog and accepts',
      (tester) async {
    final ffi = _FakeLanVaultFfi(serverRunning: true);
    final bridge = LanVaultBridge(rustFfi: ffi);
    addTearDown(bridge.dispose);

    await tester.pumpWidget(LanVaultApp(bridge: bridge));
    await tester.pumpAndSettle();

    bridge.pendingPairRequestsNotifier.value = const <PendingPairRequestModel>[
      PendingPairRequestModel(
        requestId: 'req-42',
        deviceId: 'phone-1',
        deviceName: 'Xiaomi 15',
        osType: 'Android',
        fingerprint: 'ab12cd34',
        remoteIp: '192.168.1.88',
        createdAt: 1700000000,
        expiresAt: 1700000120,
      ),
    ];
    await tester.pumpAndSettle();

    expect(find.text('收到设备配对请求'), findsOneWidget);
    expect(find.text('Xiaomi 15'), findsOneWidget);

    await tester.tap(find.text('同意配对'));
    await tester.pumpAndSettle();

    expect(ffi.pairResponses, <(String, bool)>[('req-42', true)]);
    NeuToast.dismiss();
    await tester.pump(const Duration(milliseconds: 50));
  });

  testWidgets(
      'LanVaultShell keeps dialog open and reports error when reject fails',
      (tester) async {
    final ffi = _ThrowingPairRejectFfi();
    final bridge = LanVaultBridge(rustFfi: ffi);
    addTearDown(bridge.dispose);

    await tester.pumpWidget(LanVaultApp(bridge: bridge));
    await tester.pumpAndSettle();

    bridge.pendingPairRequestsNotifier.value = const <PendingPairRequestModel>[
      PendingPairRequestModel(
        requestId: 'req-fail-1',
        deviceId: 'phone-2',
        deviceName: 'Pixel 9',
        osType: 'Android',
        fingerprint: 'ef56ab78',
        remoteIp: '192.168.1.99',
        createdAt: 1700000000,
        expiresAt: 1700000120,
      ),
    ];
    await tester.pumpAndSettle();

    await tester.tap(find.text('拒绝'));
    await tester.pumpAndSettle();

    expect(find.text('收到设备配对请求'), findsOneWidget);
    expect(find.textContaining('已拒绝 Pixel 9'), findsNothing);
    NeuToast.dismiss();
    await tester.pump(const Duration(milliseconds: 50));
  });

  testWidgets(
      'VaultScreen displays error state instead of empty state when load fails',
      (tester) async {
    final ffi = _FailingVaultFfi();
    final bridge = LanVaultBridge(rustFfi: ffi);
    addTearDown(bridge.dispose);

    await tester.pumpWidget(
      MaterialApp(
        theme: buildLanVaultTheme(),
        home: Scaffold(body: VaultScreen(bridge: bridge)),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('加载存储池失败'), findsOneWidget);
    expect(find.text('还没有存储池'), findsNothing);
    expect(find.widgetWithText(NeuButton, '重试'), findsOneWidget);
    NeuToast.dismiss();
    await tester.pump(const Duration(milliseconds: 50));
  });

  testWidgets(
      'SettingsScreen retains succeeded fields when one setting fails to load',
      (tester) async {
    final ffi = _PartialFailSettingsFfi();
    final bridge = LanVaultBridge(rustFfi: ffi);
    addTearDown(bridge.dispose);

    await tester.pumpWidget(
      MaterialApp(
        theme: buildLanVaultTheme(),
        home: Scaffold(body: SettingsScreen(bridge: bridge)),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('仅客户端'), findsOneWidget);
    expect(find.textContaining('部分配置读取失败'), findsOneWidget);
    NeuToast.dismiss();
    await tester.pump(const Duration(milliseconds: 50));
  });

  testWidgets(
      'SnapshotsScreen ignores stale _loadData response from earlier filter',
      (tester) async {
    final ffi = _DelayedSnapshotsFfi();
    final bridge = LanVaultBridge(rustFfi: ffi);
    addTearDown(bridge.dispose);

    await tester.pumpWidget(
      MaterialApp(
        theme: buildLanVaultTheme(),
        home: Scaffold(body: SnapshotsScreen(bridge: bridge)),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('全部存储池'), findsOneWidget);
    await tester.tap(find.text('全部存储池'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Vault B').last);
    await tester.pump();

    ffi.completeVaultB(<SnapshotSummaryModel>[
      const SnapshotSummaryModel(
        snapshotId: 'snap-b-latest',
        vaultId: 'v-b',
        deviceId: 'dev-1',
        retentionTier: 'daily',
        fileCount: 2,
        totalBytes: 2048,
        createdAt: '2026-11-25 10:00',
      ),
    ]);
    await tester.pumpAndSettle();
    expect(find.text('snap-b-latest'), findsOneWidget);
    NeuToast.dismiss();
    await tester.pump(const Duration(milliseconds: 50));
  });

  testWidgets('appearance mode and skin update semantic theme tokens',
      (tester) async {
    final controller = AppearanceController.memory(
      themeMode: ThemeModePreference.light,
      observeSystem: false,
    );
    addTearDown(controller.dispose);
    final light = controller.themeData.extension<NeuTheme>()!;
    expect(light.brightness, Brightness.light);
    expect(light.skinId, defaultSkinId);

    controller.setThemeMode(ThemeModePreference.dark);
    controller.setSkin(terracottaSkin);
    final dark = controller.themeData.extension<NeuTheme>()!;
    expect(dark.brightness, Brightness.dark);
    expect(dark.skinId, terracottaSkin.id);
    expect(dark.canvas, terracottaSkin.dark.canvas);
    expect(dark.surface, terracottaSkin.dark.surface);

    await tester.pumpWidget(
      MaterialApp(
        theme: controller.themeData,
        home: Builder(
          builder: (context) => Scaffold(
            backgroundColor: NeuTheme.of(context).canvas,
          ),
        ),
      ),
    );
    expect(
      tester.widget<Scaffold>(find.byType(Scaffold)).backgroundColor,
      terracottaSkin.dark.canvas,
    );
    expect(tester.takeException(), isNull);
  });
  test('built-in skin primary actions meet WCAG AA contrast', () {
    for (final skin in builtInSkins) {
      for (final palette in <SkinPalette>[skin.light, skin.dark]) {
        expect(
          contrastRatio(palette.onAccent, palette.accent),
          greaterThanOrEqualTo(4.5),
          reason: '${skin.name} ${palette == skin.dark ? 'dark' : 'light'}',
        );
      }
    }
  });

  // Regression guard for the mixColor channel/negative-amount bugs: a clamped
  // negative used to return the canvas unchanged, collapsing every light
  // palette to white-on-white.
  test('built-in skin body ink stays readable on every palette', () {
    for (final skin in builtInSkins) {
      for (final palette in <SkinPalette>[skin.light, skin.dark]) {
        final label = '${skin.name} ${palette == skin.dark ? 'dark' : 'light'}';
        expect(contrastRatio(palette.textPrimary, palette.canvas),
            greaterThanOrEqualTo(4.5), reason: 'textPrimary $label');
        expect(contrastRatio(palette.textSecondary, palette.surface),
            greaterThanOrEqualTo(4.5), reason: 'textSecondary $label');
      }
    }
  });

  testWidgets('slide viewport settles on the latest rapid selection',
      (tester) async {
    var selectedIndex = 0;
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLanVaultTheme(),
        home: StatefulBuilder(
          builder: (context, setState) => Scaffold(
            body: SlideSwitchViewport(
              index: selectedIndex,
              children: const <Widget>[
                Text('page zero'),
                Text('page one'),
                Text('page two'),
              ],
            ),
          ),
        ),
      ),
    );
    final state = tester.state<State<StatefulBuilder>>(
      find.byType(StatefulBuilder),
    );
    (state as dynamic).setState(() => selectedIndex = 1);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 30));
    (state as dynamic).setState(() => selectedIndex = 2);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 30));
    await tester.pumpAndSettle();
    final viewport = tester.widget<SlideSwitchViewport>(
      find.byType(SlideSwitchViewport),
    );
    expect(viewport.index, 2);
    expect(find.text('page two'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('reduced motion removes slide displacement', (tester) async {
    var selectedIndex = 0;
    await tester.pumpWidget(
      MaterialApp(
        theme: buildLanVaultTheme(),
        home: MediaQuery(
          data: const MediaQueryData(disableAnimations: true),
          child: StatefulBuilder(
            builder: (context, setState) {
              return Scaffold(
                body: SlideSwitchViewport(
                  index: selectedIndex,
                  children: const <Widget>[
                    Text('first'),
                    Text('second'),
                  ],
                ),
              );
            },
          ),
        ),
      ),
    );
    final state = tester.state<State<StatefulBuilder>>(
      find.byType(StatefulBuilder),
    );
    (state as dynamic).setState(() => selectedIndex = 1);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('second'), findsOneWidget);
    final viewport = tester.widget<SlideSwitchViewport>(
      find.byType(SlideSwitchViewport),
    );
    expect(viewport.index, 1);
    expect(tester.takeException(), isNull);
  });
}

class _ThrowingPairRejectFfi extends _FakeLanVaultFfi {
  _ThrowingPairRejectFfi() : super(serverRunning: true);

  @override
  Future<bool> respondPairRequest({
    required String requestId,
    required bool accept,
  }) async {
    throw StateError('network timeout');
  }
}

class _FailingVaultFfi extends _FakeLanVaultFfi {
  @override
  Future<List<VaultSummaryModel>> listVaults() async {
    throw StateError('database unavailable');
  }
}

class _PartialFailSettingsFfi extends _FakeLanVaultFfi {
  @override
  Future<RoleMode> getRoleMode() async => RoleMode.clientOnly;

  @override
  Future<PowerNetworkPolicyModel> getPowerNetworkPolicy() async {
    throw StateError('policy read failed');
  }
}

class _DelayedSnapshotsFfi extends _FakeLanVaultFfi {
  final Completer<List<SnapshotSummaryModel>> _vaultBCompleter =
      Completer<List<SnapshotSummaryModel>>();

  void completeVaultB(List<SnapshotSummaryModel> list) {
    if (!_vaultBCompleter.isCompleted) {
      _vaultBCompleter.complete(list);
    }
  }

  @override
  Future<List<VaultSummaryModel>> listVaults() async =>
      const <VaultSummaryModel>[
        VaultSummaryModel(
          vaultId: 'v-b',
          name: 'Vault B',
          poolMode: 'single',
          quotaBytes: 1024 * 1024 * 1024,
          usedBytes: 1024,
          e2eeEnabled: true,
          volumes: <VaultVolumeModel>[],
        ),
      ];

  @override
  Future<List<SnapshotSummaryModel>> listSnapshots({String? vaultId}) {
    if (vaultId == 'v-b') {
      return _vaultBCompleter.future;
    }
    return Future<List<SnapshotSummaryModel>>.value(
        const <SnapshotSummaryModel>[]);
  }
}
