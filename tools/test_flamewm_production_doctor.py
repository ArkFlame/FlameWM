import unittest
from flamewm_production_doctor import outputs_from_xrandr

class ProductionDoctorTests(unittest.TestCase):
    def test_connected_displays_and_primary(self):
        raw='eDP-1 connected primary 1920x1080+0+0 (normal left inverted right x axis y axis)\nHDMI-1 connected 2560x1440+1920+0 (normal)\nDP-2 disconnected (normal)\n'
        got=outputs_from_xrandr(raw)
        self.assertEqual(len(got), 2)
        self.assertEqual(got[0]['geometry'], [0,0,1920,1080])
        self.assertEqual(got[1]['geometry'], [1920,0,2560,1440])
        self.assertTrue(got[0]['primary'])
        self.assertFalse(got[1]['primary'])
    def test_negative_coordinates(self):
        got=outputs_from_xrandr('HDMI-1 connected primary 1920x1080+-1920+0 (normal)\n')
        self.assertEqual(got[0]['geometry'],[-1920,0,1920,1080])

if __name__=='__main__': unittest.main()
