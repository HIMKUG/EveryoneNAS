import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import 'package:lanvault_app/bridge/lanvault_bridge.dart';
import 'package:lanvault_app/bridge/system_env_helper.dart';
import 'package:lanvault_app/features/backup/backup_screen.dart';
import 'package:lanvault_app/features/vault/vault_screen.dart';
import 'package:lanvault_app/main.dart';
import 'package:lanvault_app/theme/neu_theme.dart';
import 'package:lanvault_app/theme/neu_toast.dart';

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
    required String poolMode,
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
  Future<void> importVaultKeyBackup(String passphrase, String backupJson) async {}

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
    expect(decoration.color, NeuTokens.disabledDarkSurface);
    expect(decoration.gradient, isNull);

    await tester.tap(find.text('新建存储池'));
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

    await tester.tap(find.text('新建存储池'));
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.text('请先启动本地存储服务后再新建存储池'), findsOneWidget);
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
}
